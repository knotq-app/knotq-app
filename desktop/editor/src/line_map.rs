use std::ops::Range;

use gpui::{point, px, Pixels, Point, WrappedLine};

/// A deliberately small, Vec-backed version of Monocurl's `LineMap`.
///
/// It maps logical source lines to their wrapped visual height. This is the
/// core structure we need before adding KnotQ-specific display rows such as
/// checkboxes and date annotations.
pub struct LineMap {
    lines: Vec<SchemeItemLine>,
    /// Cumulative height above each row: `offsets[row]` is the y of `row`'s top,
    /// and the last entry is the total height. Always `lines.len() + 1` long.
    ///
    /// Summing the rows above on demand made every y lookup O(row), and paint
    /// asks for one per row — so drawing a document was quadratic in its line
    /// count, ~10% of a frame on a 2,000-line scheme and rising with the square.
    /// Rebuilt by `finish_update` once relayout has written its rows, so it
    /// cannot drift from the heights it summarises.
    offsets: Vec<Pixels>,
    default_line_height: Pixels,
}

#[derive(Clone)]
pub struct SchemeItemLine {
    pub text: WrappedLine,
    pub annotation: Option<SchemeItemAnnotation>,
    pub media_height: Pixels,
    /// Extra vertical space reserved below the row for block inlines such as
    /// tables. The editor paints the block; the line map owns vertical flow.
    pub block_height: Pixels,
    /// Optional text that renders after a block inline while remaining part of
    /// the same logical source row.
    pub block_suffix: Option<WrappedLine>,
    pub block_suffix_gap: Pixels,
    /// Rows placed inside a table grid are positioned by explicit cell slots and
    /// do not contribute to normal document height.
    in_grid: bool,
    /// Number of synthetic, layout-only bytes prepended to `text` (e.g. the
    /// hanging-wrap prefix). These occupy visual space but map to buffer col 0.
    prefix_len: usize,
    /// Buffer-coordinate ranges that exist in the source line but were collapsed
    /// out of `text` (hidden markdown markers). Sorted and disjoint.
    collapsed: Vec<Range<usize>>,
    /// Length of the underlying source line in bytes (the column count), which
    /// stays constant whether or not markers are collapsed.
    buffer_len: usize,
    line_height: Pixels,
}

#[derive(Clone, Debug)]
pub struct SchemeItemAnnotation {
    pub text: String,
    pub height: Pixels,
}

impl SchemeItemLine {
    pub fn new(
        text: WrappedLine,
        annotation: Option<SchemeItemAnnotation>,
        line_height: Pixels,
    ) -> Self {
        let buffer_len = text.len();
        Self {
            text,
            annotation,
            media_height: px(0.0),
            block_height: px(0.0),
            block_suffix: None,
            block_suffix_gap: px(0.0),
            in_grid: false,
            prefix_len: 0,
            collapsed: Vec::new(),
            buffer_len,
            line_height,
        }
    }

    pub fn with_media_height(mut self, media_height: Pixels) -> Self {
        self.media_height = media_height;
        self
    }

    pub fn with_block_height(mut self, block_height: Pixels) -> Self {
        self.block_height = block_height;
        self
    }

    pub fn with_block_suffix(mut self, suffix: Option<WrappedLine>, gap: Pixels) -> Self {
        self.block_suffix = suffix;
        self.block_suffix_gap = if self.block_suffix.is_some() {
            gap
        } else {
            px(0.0)
        };
        self
    }

    pub fn in_grid(mut self, in_grid: bool) -> Self {
        self.in_grid = in_grid;
        self
    }

    /// Records how `text` (the shaped layout) relates to the source line:
    /// `prefix_len` synthetic bytes at the front, `collapsed` buffer ranges
    /// removed from the layout, and `buffer_len` source columns total.
    pub fn with_layout_mapping(
        mut self,
        prefix_len: usize,
        collapsed: Vec<Range<usize>>,
        buffer_len: usize,
    ) -> Self {
        self.prefix_len = prefix_len.min(self.text.len());
        self.collapsed = collapsed;
        self.buffer_len = buffer_len;
        self
    }

    pub(crate) fn line_height(&self) -> Pixels {
        self.line_height
    }

    pub(crate) fn height(&self) -> Pixels {
        if self.in_grid {
            return px(0.0);
        }
        self.text.size(self.line_height).height
            + self
                .annotation
                .as_ref()
                .map(|annotation| annotation.height)
                .unwrap_or(px(0.0))
            + self.media_height
            + self.block_height
            + self
                .block_suffix
                .as_ref()
                .map(|suffix| self.block_suffix_gap + suffix.size(self.line_height).height)
                .unwrap_or(px(0.0))
    }

    fn text_height(&self) -> Pixels {
        self.text.size(self.line_height).height
    }

    fn visible_len(&self) -> usize {
        self.buffer_len
    }

    /// Buffer bytes collapsed strictly before `col`. If `col` falls inside a
    /// collapsed range, it is clamped to that range's start (its layout point).
    fn collapsed_before(&self, col: usize) -> usize {
        let mut removed = 0;
        for range in &self.collapsed {
            if range.end <= col {
                removed += range.end - range.start;
            } else if range.start < col {
                removed += col - range.start;
                break;
            } else {
                break;
            }
        }
        removed
    }

    /// Buffer spans that remain in the layout (the complement of `collapsed`).
    fn kept_spans(&self) -> Vec<Range<usize>> {
        let mut spans = Vec::with_capacity(self.collapsed.len() + 1);
        let mut pos = 0;
        for range in &self.collapsed {
            let start = range.start.min(self.buffer_len);
            let end = range.end.min(self.buffer_len);
            if start > pos {
                spans.push(pos..start);
            }
            pos = pos.max(end);
        }
        if pos < self.buffer_len {
            spans.push(pos..self.buffer_len);
        }
        spans
    }

    fn layout_index_for_col(&self, col: usize) -> usize {
        let col = col.min(self.buffer_len);
        self.prefix_len + col - self.collapsed_before(col)
    }

    fn visible_col_for_layout_index(&self, index: usize) -> usize {
        if index <= self.prefix_len {
            return 0;
        }
        let mut compacted = index - self.prefix_len;
        for span in self.kept_spans() {
            let len = span.end - span.start;
            if compacted <= len {
                return span.start + compacted;
            }
            compacted -= len;
        }
        self.buffer_len
    }

    fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        self.text
            .position_for_index(self.layout_index_for_col(index), self.line_height)
    }

    fn closest_index_for_position(&self, position: Point<Pixels>) -> usize {
        let col = match self
            .text
            .closest_index_for_position(position, self.line_height)
        {
            Ok(col) | Err(col) => col,
        };
        self.visible_col_for_layout_index(col)
    }

    fn wrapped_line_ranges(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::with_capacity(self.text.wrap_boundaries().len() + 1);
        let mut start = 0;
        for boundary in self.text.wrap_boundaries() {
            let run = &self.text.runs()[boundary.run_ix];
            let glyph = &run.glyphs[boundary.glyph_ix];
            let end = glyph.index.min(self.text.len());
            ranges.push(
                self.visible_col_for_layout_index(start)..self.visible_col_for_layout_index(end),
            );
            start = end;
        }
        ranges.push(
            self.visible_col_for_layout_index(start)
                ..self.visible_col_for_layout_index(self.text.len()),
        );
        ranges
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextLocation {
    pub row: usize,
    pub col: usize,
}

impl Ord for TextLocation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.row
            .cmp(&other.row)
            .then_with(|| self.col.cmp(&other.col))
    }
}

impl PartialOrd for TextLocation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl LineMap {
    pub fn new(line_height: Pixels) -> Self {
        Self {
            lines: Vec::new(),
            offsets: vec![px(0.0)],
            default_line_height: line_height,
        }
    }

    /// Make the map hold exactly `rows` rows, keeping the ones already there at
    /// the same indices. New rows are placeholders until `set_line` fills them.
    ///
    /// Relayout updates the map IN PLACE rather than rebuilding it. A
    /// `SchemeItemLine` carries its shaped decoration runs inline and is ~1.4KB,
    /// so handing the whole vector out and building a new one moved every row's
    /// worth of that on every keystroke — 42MB of memcpy per keypress in a
    /// 10,000-line scheme, and the single largest cost left in relayout. Rows
    /// whose shaping inputs are unchanged are now simply not touched.
    pub fn resize_rows(&mut self, rows: usize) {
        self.lines.resize_with(rows, Self::placeholder);
    }

    /// Replace the rows `at..at + removed` with `inserted` placeholders,
    /// shifting the rows after them without re-shaping any of them.
    ///
    /// Rows are addressed by index, so inserting a line renumbers every row
    /// below it. Treating that as "every later row changed" made pressing Enter
    /// near the top of a large document re-shape the whole thing; moving them is
    /// one memmove. The caller must apply the SAME splice to the shape cache, or
    /// the two disagree about which line belongs to which row.
    pub fn splice_rows(&mut self, at: usize, removed: usize, inserted: usize) {
        let at = at.min(self.lines.len());
        let removed = removed.min(self.lines.len() - at);
        self.lines.splice(
            at..at + removed,
            std::iter::repeat_with(Self::placeholder).take(inserted),
        );
    }

    /// A row that exists but has not been shaped yet. Zero height, so it takes
    /// no space if it is somehow painted before relayout fills it in.
    fn placeholder() -> SchemeItemLine {
        SchemeItemLine::new(WrappedLine::default(), None, px(0.0))
    }

    pub fn set_line(&mut self, row: usize, line: SchemeItemLine) {
        if let Some(slot) = self.lines.get_mut(row) {
            *slot = line;
        }
    }

    /// Recompute the row offsets after an update. Must be called once relayout
    /// has finished writing rows, or every y is stale.
    pub fn finish_update(&mut self) {
        self.offsets.clear();
        self.offsets.reserve(self.lines.len() + 1);
        let mut y = px(0.0);
        self.offsets.push(y);
        for line in &self.lines {
            y += line.height();
            self.offsets.push(y);
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line_height(&self) -> Pixels {
        self.default_line_height
    }

    pub fn row_line_height(&self, row: usize) -> Pixels {
        self.lines
            .get(row)
            .map(SchemeItemLine::line_height)
            .unwrap_or(self.default_line_height)
    }

    pub fn line(&self, row: usize) -> Option<&WrappedLine> {
        self.lines.get(row).map(|line| &line.text)
    }

    pub fn item_line(&self, row: usize) -> Option<&SchemeItemLine> {
        self.lines.get(row)
    }

    pub fn line_len(&self, row: usize) -> usize {
        self.lines
            .get(row)
            .map(SchemeItemLine::visible_len)
            .unwrap_or(0)
    }

    pub fn line_text_height(&self, row: usize) -> Pixels {
        self.lines
            .get(row)
            .map(SchemeItemLine::text_height)
            .unwrap_or(self.default_line_height)
    }

    pub fn total_height(&self) -> Pixels {
        self.offsets[self.lines.len()]
    }

    pub fn y_range(&self, rows: Range<usize>) -> Range<Pixels> {
        let start = self.height_before(rows.start);
        let end = self.height_before(rows.end);
        start..end
    }

    /// The rows whose vertical band intersects `band`, in content coordinates.
    ///
    /// Both ends are found by searching the offsets rather than walking, so
    /// asking "what is on screen?" costs the same in a 10-line document and a
    /// 10,000-line one.
    pub fn rows_intersecting(&self, band: Range<Pixels>) -> Range<usize> {
        if self.lines.is_empty() {
            return 0..0;
        }
        // First row whose bottom reaches into the band...
        let first = self.offsets[1..].partition_point(|bottom| *bottom < band.start);
        // ...through the last row whose top is still inside it.
        let last = self.offsets[..self.lines.len()].partition_point(|top| *top <= band.end);
        first..last.max(first)
    }

    pub fn point_for_location(&self, location: TextLocation) -> Point<Pixels> {
        if self.lines.is_empty() {
            return point(px(0.0), px(0.0));
        }

        let row = location.row.min(self.lines.len().saturating_sub(1));
        let y = self.height_before(row);
        let x = self.lines[row]
            .position_for_index(location.col)
            .map(|p| p.x)
            .unwrap_or(px(0.0));

        let local_y = self.lines[row]
            .position_for_index(location.col)
            .map(|p| p.y)
            .unwrap_or(px(0.0));

        point(x, y + local_y)
    }

    pub fn location_for_point(&self, pos: Point<Pixels>) -> TextLocation {
        if self.lines.is_empty() {
            return TextLocation { row: 0, col: 0 };
        }

        if pos.y < px(0.0) {
            return TextLocation { row: 0, col: 0 };
        }

        // The first row whose bottom is past `pos.y`. Rows are laid out
        // top-to-bottom, so `offsets` is non-decreasing and can be searched
        // rather than walked — a zero-height row keeps the same tie-break as
        // the old scan (the first such row wins) because `partition_point`
        // returns the first index whose bottom strictly exceeds `pos.y`.
        let row = self.offsets[1..].partition_point(|bottom| *bottom <= pos.y);
        if let Some(line) = self.lines.get(row) {
            let local_y = pos.y - self.offsets[row];
            let col = if local_y < line.text_height() {
                line.closest_index_for_position(point(pos.x, local_y))
            } else {
                line.visible_len()
            };
            return TextLocation {
                row,
                col: col.min(line.visible_len()),
            };
        }

        let row = self.lines.len().saturating_sub(1);
        TextLocation {
            row,
            col: self.line_len(row),
        }
    }

    pub fn position_for_index(&self, row: usize, index: usize) -> Option<Point<Pixels>> {
        self.lines
            .get(row)
            .and_then(|line| line.position_for_index(index))
    }

    pub fn closest_col(&self, row: usize, local: Point<Pixels>) -> usize {
        self.lines
            .get(row)
            .map(|line| line.closest_index_for_position(local))
            .unwrap_or(0)
    }

    pub fn wrapped_line_ranges(&self, row: usize) -> Vec<Range<usize>> {
        self.lines
            .get(row)
            .map(SchemeItemLine::wrapped_line_ranges)
            .unwrap_or_default()
    }

    fn height_before(&self, row: usize) -> Pixels {
        self.offsets[row.min(self.lines.len())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(heights: &[f32]) -> LineMap {
        let mut map = LineMap::new(px(10.0));
        map.resize_rows(heights.len());
        for (row, height) in heights.iter().enumerate() {
            map.set_line(
                row,
                SchemeItemLine::new(Default::default(), None, px(*height)),
            );
        }
        map.finish_update();
        map
    }

    /// What `location_for_point` did before it became a binary search: walk the
    /// rows top-down and take the first whose bottom is past `y`. The search
    /// must agree with it everywhere, including on zero-height rows and past
    /// the end of the document.
    fn row_by_scan(heights: &[f32], y: f32) -> Option<usize> {
        let mut top = 0.0;
        for (row, height) in heights.iter().enumerate() {
            if y < top + height {
                return Some(row);
            }
            top += height;
        }
        None
    }

    #[test]
    fn hit_testing_a_y_matches_a_top_down_scan() {
        // Includes zero-height rows, which is where "first row whose bottom is
        // past y" and "last row whose top is at or before y" disagree.
        let heights = [12.0, 0.0, 30.0, 0.0, 0.0, 8.0, 25.0];
        let map = map_of(&heights);

        for tenth in -20..1000 {
            let y = tenth as f32 / 10.0;
            let expected = row_by_scan(&heights, y).unwrap_or(heights.len() - 1);
            assert_eq!(
                map.location_for_point(point(px(0.0), px(y))).row,
                expected,
                "y = {y}"
            );
        }
    }

    #[test]
    fn an_empty_map_hit_tests_to_the_origin() {
        let map = LineMap::new(px(10.0));

        assert_eq!(
            map.location_for_point(point(px(5.0), px(50.0))),
            TextLocation { row: 0, col: 0 }
        );
    }

    #[test]
    fn row_offsets_and_total_height_add_up() {
        let map = map_of(&[12.0, 0.0, 30.0, 8.0]);

        assert_eq!(map.y_range(0..1), px(0.0)..px(12.0));
        assert_eq!(map.y_range(1..2), px(12.0)..px(12.0));
        assert_eq!(map.y_range(2..4), px(12.0)..px(50.0));
        assert_eq!(map.total_height(), px(50.0));
        // Out of range clamps to the end rather than panicking.
        assert_eq!(map.y_range(9..12), px(50.0)..px(50.0));
    }

    /// Shrinking must drop the rows that went away, and the offsets with them —
    /// a stale tail would place the caret and hit-testing past the end.
    #[test]
    fn resizing_smaller_drops_the_rows_that_went_away() {
        let mut map = map_of(&[12.0, 30.0, 8.0]);
        map.resize_rows(1);
        map.finish_update();

        assert_eq!(map.line_count(), 1);
        assert_eq!(map.total_height(), px(12.0));
    }

    /// Growing must keep the existing rows at the SAME indices — relayout
    /// relies on that to decide a row is unchanged and leave it alone.
    #[test]
    fn resizing_larger_keeps_existing_rows_in_place() {
        let mut map = map_of(&[12.0, 30.0]);
        map.resize_rows(4);
        map.set_line(3, SchemeItemLine::new(Default::default(), None, px(5.0)));
        map.finish_update();

        assert_eq!(map.line_count(), 4);
        assert_eq!(map.y_range(0..1), px(0.0)..px(12.0));
        assert_eq!(map.y_range(1..2), px(12.0)..px(42.0));
        // The untouched new row is a zero-height placeholder.
        assert_eq!(map.y_range(2..3), px(42.0)..px(42.0));
        assert_eq!(map.total_height(), px(47.0));
    }

    /// A splice must move the surviving rows rather than disturb them: that is
    /// what lets relayout keep their shaped lines when a line is inserted.
    #[test]
    fn splicing_shifts_the_rows_after_the_change() {
        let mut map = map_of(&[12.0, 30.0, 8.0]);

        // Insert one row at the top.
        map.splice_rows(0, 0, 1);
        map.finish_update();
        assert_eq!(map.line_count(), 4);
        // The new row is a zero-height placeholder; the old rows kept their
        // heights, just one index later.
        assert_eq!(map.y_range(0..1), px(0.0)..px(0.0));
        assert_eq!(map.y_range(1..2), px(0.0)..px(12.0));
        assert_eq!(map.y_range(2..3), px(12.0)..px(42.0));
        assert_eq!(map.total_height(), px(50.0));

        // Remove it again and the original layout is back.
        map.splice_rows(0, 1, 0);
        map.finish_update();
        assert_eq!(map.line_count(), 3);
        assert_eq!(map.y_range(0..1), px(0.0)..px(12.0));
        assert_eq!(map.total_height(), px(50.0));
    }

    /// A splice past the end must clamp rather than panic — the row list and
    /// the buffer are built together, but a disagreement must not crash.
    #[test]
    fn splicing_past_the_end_clamps() {
        let mut map = map_of(&[12.0]);
        map.splice_rows(9, 9, 2);
        map.finish_update();

        assert_eq!(map.line_count(), 3);
    }

    /// The visible band must name exactly the rows a top-down scan would find
    /// overlapping it — a row wrongly excluded is a row that does not draw.
    #[test]
    fn the_visible_band_matches_a_top_down_scan() {
        let heights = [12.0, 0.0, 30.0, 8.0, 25.0];
        let map = map_of(&heights);

        for start in 0..80 {
            for len in 0..80 {
                let (from, to) = (start as f32, (start + len) as f32);
                let expected: Vec<usize> = {
                    let mut rows = Vec::new();
                    let mut top = 0.0f32;
                    for (row, height) in heights.iter().enumerate() {
                        let bottom = top + height;
                        if bottom >= from && top <= to {
                            rows.push(row);
                        }
                        top = bottom;
                    }
                    rows
                };
                let band = map.rows_intersecting(px(from)..px(to));
                let got: Vec<usize> = band.collect();
                assert_eq!(got, expected, "band {from}..{to}");
            }
        }
    }

    #[test]
    fn an_empty_map_has_no_visible_rows() {
        let map = LineMap::new(px(10.0));

        assert_eq!(map.rows_intersecting(px(0.0)..px(100.0)), 0..0);
    }

    /// Finding the visible rows must not depend on how long the document is —
    /// that is the whole reason it is a search and not a walk.
    ///
    /// Both sides perform the same 20,000 searches, so only the document size
    /// differs. See [`fastest`] for why this takes a minimum rather than a
    /// single sample.
    #[test]
    fn finding_the_visible_rows_does_not_scale_with_the_document() {
        fn searches(rows: usize) -> std::time::Duration {
            let map = map_of(&vec![10.0; rows]);
            fastest(5, || {
                for offset in 0..20_000 {
                    let top = px(offset as f32);
                    std::hint::black_box(map.rows_intersecting(top..top + px(500.0)));
                }
            })
        }

        let small = searches(1_000).max(std::time::Duration::from_nanos(1));
        let large = searches(64_000);

        // Equal work. A binary search costs ~1.6x more per search over 64x the
        // rows (six extra steps on about ten); a walk would cost ~64x.
        assert!(
            large < small * 8,
            "finding visible rows scales with the document: {small:?} over 1,000 rows \
             vs {large:?} over 64,000, for the same 20,000 searches"
        );
    }

    /// Asking a row for its y must cost the same whatever the document's size.
    ///
    /// Summing the rows above on demand made this O(rows) per row, so painting
    /// a document was quadratic in its line count — ~10% of a frame at 2,000
    /// lines and growing with the square.
    ///
    /// Both sides do the SAME NUMBER of lookups, so a constant-time lookup
    /// makes them equal and only the document size differs. That is what makes
    /// the bound meaningful: an earlier version compared 1,000 rows against
    /// 8,000 and called linear "under 24x", which is a fudge factor hiding the
    /// fact that it was really measuring two different amounts of work — and it
    /// duly flaked on a macOS runner (20µs vs 635µs).
    #[test]
    fn a_row_y_lookup_costs_the_same_at_any_document_size() {
        fn lookups(rows: usize, passes: usize) -> std::time::Duration {
            let map = map_of(&vec![10.0; rows]);
            fastest(5, || {
                let mut total = px(0.0);
                for _ in 0..passes {
                    for row in 0..rows {
                        total += map.y_range(row..row + 1).start;
                    }
                }
                std::hint::black_box(total);
            })
        }

        // 64,000 lookups either way.
        let small = lookups(1_000, 64).max(std::time::Duration::from_nanos(1));
        let large = lookups(64_000, 1);

        // Equal work, so a prefix sum makes these roughly equal; the 64,000-row
        // array has worse locality, hence the headroom. The per-row sum this
        // replaced would make `large` ~64x slower.
        assert!(
            large < small * 8,
            "a row's y lookup scales with the document: {small:?} for 64,000 lookups \
             over 1,000 rows vs {large:?} for the same 64,000 over 64,000 rows"
        );
    }

    /// The shortest of `trials` runs of `run`.
    ///
    /// A ratio between two timings only means anything if neither run was
    /// preempted, and on a shared CI runner some run always is. The minimum is
    /// the one that came closest to having the machine to itself; a mean is
    /// dragged around by whatever else was running.
    fn fastest(trials: usize, mut run: impl FnMut()) -> std::time::Duration {
        // Warm the allocator and the branch predictor first, so the timed runs
        // are not the ones paying for them.
        run();
        (0..trials)
            .map(|_| {
                let start = std::time::Instant::now();
                run();
                start.elapsed()
            })
            .min()
            .unwrap_or_default()
    }
}
