use super::super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn current_row_index(&self) -> usize {
        self.selection
            .head
            .row
            .min(self.render_line_count().saturating_sub(1))
    }

    pub(in crate::scheme_editor) fn selected_row_range(&self) -> (usize, usize) {
        let (start, end) = self.selection.ordered();
        let last = self.render_line_count().saturating_sub(1);
        (start.row.min(last), end.row.min(last))
    }

    pub(in crate::scheme_editor) fn emit_commands(&mut self, commands: Vec<Command>, cx: &mut Context<Self>) {
        if let Some(cmd) = Command::from_vec(commands) {
            cx.emit(EditorEvent::Command(cmd));
            self.reset_cursor_blink(cx);
            cx.notify();
        }
    }

    pub(in crate::scheme_editor) fn clamp_location(&self, loc: TextLocation) -> TextLocation {
        let row_count = self.render_line_count();
        let row = loc.row.min(row_count.saturating_sub(1));
        let col = loc.col.min(self.line_len(row));
        TextLocation { row, col }
    }

    pub(in crate::scheme_editor) fn render_line_count(&self) -> usize {
        self.text.line_count().max(1)
    }

    pub(in crate::scheme_editor) fn line_len(&self, row: usize) -> usize {
        self.text.line_len(row)
    }

    /// Column of the first non-whitespace character on `row`, or `0` if the
    /// line is empty/all-whitespace. Used to make Home skip leading
    /// indentation instead of always landing on column 0.
    pub(in crate::scheme_editor) fn first_non_whitespace_column(&self, row: usize) -> usize {
        let Some(range) = self.line_range(row) else {
            return 0;
        };
        self.text
            .get(range)
            .and_then(|line| {
                line.char_indices()
                    .find_map(|(col, ch)| (!ch.is_whitespace()).then_some(col))
            })
            .unwrap_or(0)
    }

    pub(in crate::scheme_editor) fn line_range(&self, row: usize) -> Option<Range<usize>> {
        self.text.line_range(row)
    }

    pub(in crate::scheme_editor) fn table_object_range_for_row(&self, row: usize) -> Option<Range<usize>> {
        let editor_row = self.rows.get(row)?;
        if !editor_row.path.is_table_anchor() || !editor_row.item.has_table() {
            return None;
        }
        let range = self.line_range(row)?;
        table_object_range(self.text.get(range)?)
    }

    pub(in crate::scheme_editor) fn location_to_offset(&self, loc: TextLocation) -> usize {
        location_to_offset_with(&self.text, self.text.line_ranges(), loc)
    }

    pub(in crate::scheme_editor) fn offset_to_location(&self, offset: usize) -> TextLocation {
        offset_to_location_with(self.text.len(), self.text.line_ranges(), offset)
    }

    pub(in crate::scheme_editor) fn offset_to_location_in(&self, text: &str, offset: usize) -> TextLocation {
        offset_to_location_with(text.len(), &line_ranges(text), offset)
    }

    pub(in crate::scheme_editor) fn selection_offsets(&self) -> (usize, usize) {
        let (start, end) = self.selection.ordered();
        (self.location_to_offset(start), self.location_to_offset(end))
    }

    pub(in crate::scheme_editor) fn selected_text(&self) -> Option<String> {
        if self.selection.is_empty() {
            return None;
        }
        let (start, end) = self.selection_offsets();
        self.text.get(start..end).map(line_without_table_object)
    }

    pub(in crate::scheme_editor) fn selected_whole_rows(&self) -> Option<Range<usize>> {
        let line_lens: Vec<usize> = (0..self.render_line_count())
            .map(|row| self.line_len(row))
            .collect();
        whole_row_selection_range(self.selection, &line_lens)
    }
}

/// `(row, col)` → byte offset, given the line index for `text`.
///
/// Split out from the editor so both the cached-index path and the
/// foreign-text path run exactly the same mapping; two copies of this would be
/// two chances for the caret to land somewhere different.
fn location_to_offset_with(text: &str, ranges: &[Range<usize>], loc: TextLocation) -> usize {
    if ranges.is_empty() {
        return 0;
    }
    let row = loc.row.min(ranges.len() - 1);
    let range = ranges[row].clone();
    let col = loc.col.min(range.end - range.start);
    let mut offset = range.start + col;
    while offset > range.start && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Byte offset → `(row, col)`, given the line index.
///
/// Binary search rather than a scan: this is asked once per row while
/// painting, so a linear walk made drawing quadratic in the line count.
/// `ranges` is sorted and non-overlapping, so the row is the last one whose
/// end is still at or after `offset`.
fn offset_to_location_with(len: usize, ranges: &[Range<usize>], offset: usize) -> TextLocation {
    if ranges.is_empty() {
        return TextLocation { row: 0, col: 0 };
    }
    let offset = offset.min(len);
    // First row whose end is >= offset — the same row the scan returned.
    let row = ranges.partition_point(|range| range.end < offset);
    let row = row.min(ranges.len() - 1);
    TextLocation {
        row,
        col: offset.saturating_sub(ranges[row].start),
    }
}
