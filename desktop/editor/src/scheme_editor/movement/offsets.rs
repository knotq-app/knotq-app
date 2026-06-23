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
        line_ranges(&self.text).len().max(1)
    }

    pub(in crate::scheme_editor) fn line_len(&self, row: usize) -> usize {
        let ranges = line_ranges(&self.text);
        ranges
            .get(row)
            .map(|range| range.end.saturating_sub(range.start))
            .unwrap_or(0)
    }

    pub(in crate::scheme_editor) fn line_range(&self, row: usize) -> Option<Range<usize>> {
        line_ranges(&self.text).get(row).cloned()
    }

    pub(in crate::scheme_editor) fn table_object_range_for_row(&self, row: usize) -> Option<Range<usize>> {
        let editor_row = self.rows.get(row)?;
        if !editor_row.path.is_table_anchor() || !editor_row.item.has_table() {
            return None;
        }
        let range = self.line_range(row)?;
        table_object_range(self.text.get(range)?)
    }

    pub(in crate::scheme_editor) fn text_lines(&self) -> Vec<String> {
        self.text.split('\n').map(ToString::to_string).collect()
    }

    pub(in crate::scheme_editor) fn location_to_offset(&self, loc: TextLocation) -> usize {
        self.location_to_offset_in(&self.text, loc)
    }

    pub(in crate::scheme_editor) fn location_to_offset_in(&self, text: &str, loc: TextLocation) -> usize {
        let ranges = line_ranges(text);
        if ranges.is_empty() {
            return 0;
        }
        let row = loc.row.min(ranges.len().saturating_sub(1));
        let range = ranges[row].clone();
        let col = loc.col.min(range.end - range.start);
        let mut offset = range.start + col;
        while offset > range.start && !text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    pub(in crate::scheme_editor) fn offset_to_location(&self, offset: usize) -> TextLocation {
        self.offset_to_location_in(&self.text, offset)
    }

    pub(in crate::scheme_editor) fn offset_to_location_in(&self, text: &str, offset: usize) -> TextLocation {
        let ranges = line_ranges(text);
        if ranges.is_empty() {
            return TextLocation { row: 0, col: 0 };
        }
        let offset = offset.min(text.len());
        for (row, range) in ranges.iter().enumerate() {
            if offset <= range.end {
                return TextLocation {
                    row,
                    col: offset.saturating_sub(range.start),
                };
            }
        }
        let row = ranges.len().saturating_sub(1);
        TextLocation {
            row,
            col: ranges[row].end.saturating_sub(ranges[row].start),
        }
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
