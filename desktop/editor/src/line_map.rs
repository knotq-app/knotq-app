use std::ops::Range;

use gpui::{point, px, Pixels, Point, WrappedLine};

/// A deliberately small, Vec-backed version of Monocurl's `LineMap`.
///
/// It maps logical source lines to their wrapped visual height. This is the
/// core structure we need before adding KnotQ-specific display rows such as
/// checkboxes and date annotations.
pub struct LineMap {
    lines: Vec<SchemeItemLine>,
    default_line_height: Pixels,
}

#[derive(Clone)]
pub struct SchemeItemLine {
    pub text: WrappedLine,
    pub annotation: Option<SchemeItemAnnotation>,
    pub media_height: Pixels,
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
        self.text.size(self.line_height).height
            + self
                .annotation
                .as_ref()
                .map(|annotation| annotation.height)
                .unwrap_or(px(0.0))
            + self.media_height
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
            default_line_height: line_height,
        }
    }

    pub fn replace_lines(&mut self, lines: Vec<SchemeItemLine>) {
        self.lines = lines;
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
        self.lines
            .iter()
            .fold(px(0.0), |height, line| height + line.height())
    }

    pub fn y_range(&self, rows: Range<usize>) -> Range<Pixels> {
        let start = self.height_before(rows.start);
        let end = self.height_before(rows.end);
        start..end
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

        let mut y = px(0.0);
        for (row, line) in self.lines.iter().enumerate() {
            let text_height = line.text_height();
            let height = line.height();
            if pos.y < y + height {
                let local_y = pos.y - y;
                let col = if local_y < text_height {
                    let local = point(pos.x, local_y);
                    line.closest_index_for_position(local)
                } else {
                    line.visible_len()
                };
                return TextLocation {
                    row,
                    col: col.min(line.visible_len()),
                };
            }
            y += height;
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

    pub fn wrapped_line_ranges(&self, row: usize) -> Vec<Range<usize>> {
        self.lines
            .get(row)
            .map(SchemeItemLine::wrapped_line_ranges)
            .unwrap_or_default()
    }

    fn height_before(&self, row: usize) -> Pixels {
        self.lines
            .iter()
            .take(row.min(self.lines.len()))
            .fold(px(0.0), |height, line| height + line.height())
    }
}
