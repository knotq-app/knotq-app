use std::ops::Range;

use gpui::{point, px, Pixels, Point, WrappedLine};

/// A deliberately small, Vec-backed version of Monocurl's `LineMap`.
///
/// It maps logical source lines to their wrapped visual height. This is the
/// core structure we need before adding KnotQ-specific display rows such as
/// checkboxes and date annotations.
pub struct LineMap {
    lines: Vec<SchemeItemLine>,
    line_height: Pixels,
}

#[derive(Clone)]
pub struct SchemeItemLine {
    pub text: WrappedLine,
    pub annotation: Option<SchemeItemAnnotation>,
    pub media_height: Pixels,
    hidden_prefix_len: usize,
}

#[derive(Clone, Debug)]
pub struct SchemeItemAnnotation {
    pub text: String,
    pub height: Pixels,
}

impl SchemeItemLine {
    pub fn new(text: WrappedLine, annotation: Option<SchemeItemAnnotation>) -> Self {
        Self {
            text,
            annotation,
            media_height: px(0.0),
            hidden_prefix_len: 0,
        }
    }

    pub fn with_media_height(mut self, media_height: Pixels) -> Self {
        self.media_height = media_height;
        self
    }

    pub fn with_hidden_prefix(mut self, hidden_prefix_len: usize) -> Self {
        self.hidden_prefix_len = hidden_prefix_len.min(self.text.len());
        self
    }

    pub(crate) fn height(&self, line_height: Pixels) -> Pixels {
        self.text.size(line_height).height
            + self
                .annotation
                .as_ref()
                .map(|annotation| annotation.height)
                .unwrap_or(px(0.0))
            + self.media_height
    }

    fn text_height(&self, line_height: Pixels) -> Pixels {
        self.text.size(line_height).height
    }

    fn visible_len(&self) -> usize {
        self.text.len().saturating_sub(self.hidden_prefix_len)
    }

    fn layout_index_for_col(&self, col: usize) -> usize {
        self.hidden_prefix_len + col.min(self.visible_len())
    }

    fn visible_col_for_layout_index(&self, index: usize) -> usize {
        index
            .saturating_sub(self.hidden_prefix_len)
            .min(self.visible_len())
    }

    fn position_for_index(&self, index: usize, line_height: Pixels) -> Option<Point<Pixels>> {
        self.text
            .position_for_index(self.layout_index_for_col(index), line_height)
    }

    fn closest_index_for_position(&self, position: Point<Pixels>, line_height: Pixels) -> usize {
        let col = match self.text.closest_index_for_position(position, line_height) {
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
            line_height,
        }
    }

    pub fn replace_lines(&mut self, lines: Vec<SchemeItemLine>) {
        self.lines = lines;
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line_height(&self) -> Pixels {
        self.line_height
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
            .map(|line| line.text_height(self.line_height))
            .unwrap_or(self.line_height)
    }

    pub fn total_height(&self) -> Pixels {
        self.lines.iter().fold(px(0.0), |height, line| {
            height + line.height(self.line_height)
        })
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
            .position_for_index(location.col, self.line_height)
            .map(|p| p.x)
            .unwrap_or(px(0.0));

        let local_y = self.lines[row]
            .position_for_index(location.col, self.line_height)
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
            let text_height = line.text_height(self.line_height);
            let height = line.height(self.line_height);
            if pos.y < y + height {
                let local_y = pos.y - y;
                let col = if local_y < text_height {
                    let local = point(pos.x, local_y);
                    line.closest_index_for_position(local, self.line_height)
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
            .and_then(|line| line.position_for_index(index, self.line_height))
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
            .fold(px(0.0), |height, line| {
                height + line.height(self.line_height)
            })
    }
}
