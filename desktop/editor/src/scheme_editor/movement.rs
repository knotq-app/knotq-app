use super::*;

impl SchemeEditor {
    pub(super) fn move_cursor_to(
        &mut self,
        loc: TextLocation,
        select: bool,
        cx: &mut Context<Self>,
    ) {
        let loc = self.clamp_location(loc);
        if select {
            self.selection.head = loc;
        } else {
            self.selection = TextSelection::collapsed(loc);
        }
        self.marked_range = None;
        self.cursor_blink_state = true;
        cx.emit(EditorEvent::SelectionChanged {
            scheme_id: self.scheme_id,
        });
        self.scroll_to_cursor(cx);
        cx.notify();
    }

    pub(super) fn move_left(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (start, _) = self.selection.ordered();
            self.move_cursor_to(start, false, cx);
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        let prev = previous_char_boundary(&self.text, offset);
        self.move_cursor_to(self.offset_to_location(prev), select, cx);
    }

    pub(super) fn move_right(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (_, end) = self.selection.ordered();
            self.move_cursor_to(end, false, cx);
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        let next = next_char_boundary(&self.text, offset);
        self.move_cursor_to(self.offset_to_location(next), select, cx);
    }

    pub(super) fn move_vertical(&mut self, delta: isize, select: bool, cx: &mut Context<Self>) {
        if self.line_map.line_count() > 0 {
            let current = self.visual_point_for_location(self.selection.head);
            let current_row = self.selection.head.row.min(self.line_map.line_count() - 1);
            let current_row_top = self.line_map.y_range(current_row..current_row + 1).start;
            let local_y = current.y - current_row_top;
            let target_row = (current_row as isize + delta)
                .clamp(0, self.line_map.line_count().saturating_sub(1) as isize)
                as usize;
            let target_row_top = self.line_map.y_range(target_row..target_row + 1).start;
            let target_text_height =
                (self.line_map.line_text_height(target_row) - px(1.0)).max(px(0.0));
            let target = point(current.x, target_row_top + local_y.min(target_text_height));
            self.move_cursor_to(self.location_for_local_point(target), select, cx);
        } else {
            let row_count = self.render_line_count();
            let row = (self.selection.head.row as isize + delta)
                .clamp(0, row_count.saturating_sub(1) as isize) as usize;
            let col = self.selection.head.col.min(self.line_len(row));
            self.move_cursor_to(TextLocation { row, col }, select, cx);
        }
    }

    pub(super) fn move_word_left(&mut self, select: bool, cx: &mut Context<Self>) {
        let offset = self.location_to_offset(self.selection.head);
        self.move_cursor_to(
            self.offset_to_location(previous_word_offset(&self.text, offset)),
            select,
            cx,
        );
    }

    pub(super) fn move_word_right(&mut self, select: bool, cx: &mut Context<Self>) {
        let offset = self.location_to_offset(self.selection.head);
        self.move_cursor_to(
            self.offset_to_location(next_word_offset(&self.text, offset)),
            select,
            cx,
        );
    }

    pub(super) fn move_line_start(&mut self, select: bool, cx: &mut Context<Self>) {
        self.move_cursor_to(
            TextLocation {
                row: self.selection.head.row,
                col: 0,
            },
            select,
            cx,
        );
    }

    pub(super) fn move_line_end(&mut self, select: bool, cx: &mut Context<Self>) {
        let row = self.selection.head.row;
        self.move_cursor_to(
            TextLocation {
                row,
                col: self.line_len(row),
            },
            select,
            cx,
        );
    }

    pub(super) fn move_document_start(&mut self, select: bool, cx: &mut Context<Self>) {
        self.move_cursor_to(TextLocation { row: 0, col: 0 }, select, cx);
    }

    pub(super) fn move_document_end(&mut self, select: bool, cx: &mut Context<Self>) {
        self.move_cursor_to(self.offset_to_location(self.text.len()), select, cx);
    }

    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = TextSelection {
            anchor: TextLocation { row: 0, col: 0 },
            head: self.offset_to_location(self.text.len()),
        };
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn current_row_index(&self) -> usize {
        self.selection
            .head
            .row
            .min(self.render_line_count().saturating_sub(1))
    }

    pub(super) fn selected_row_range(&self) -> (usize, usize) {
        let (start, end) = self.selection.ordered();
        let last = self.render_line_count().saturating_sub(1);
        (start.row.min(last), end.row.min(last))
    }

    pub(super) fn emit_commands(&mut self, commands: Vec<Command>, cx: &mut Context<Self>) {
        if let Some(cmd) = Command::from_vec(commands) {
            cx.emit(EditorEvent::Command(cmd));
            self.reset_cursor_blink(cx);
            cx.notify();
        }
    }

    pub(super) fn clamp_location(&self, loc: TextLocation) -> TextLocation {
        let row_count = self.render_line_count();
        let row = loc.row.min(row_count.saturating_sub(1));
        let col = loc.col.min(self.line_len(row));
        TextLocation { row, col }
    }

    pub(super) fn render_line_count(&self) -> usize {
        line_ranges(&self.text).len().max(1)
    }

    pub(super) fn line_len(&self, row: usize) -> usize {
        let ranges = line_ranges(&self.text);
        ranges
            .get(row)
            .map(|range| range.end.saturating_sub(range.start))
            .unwrap_or(0)
    }

    pub(super) fn line_range(&self, row: usize) -> Option<Range<usize>> {
        line_ranges(&self.text).get(row).cloned()
    }

    pub(super) fn text_lines(&self) -> Vec<String> {
        self.text.split('\n').map(ToString::to_string).collect()
    }

    pub(super) fn location_to_offset(&self, loc: TextLocation) -> usize {
        self.location_to_offset_in(&self.text, loc)
    }

    pub(super) fn location_to_offset_in(&self, text: &str, loc: TextLocation) -> usize {
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

    pub(super) fn offset_to_location(&self, offset: usize) -> TextLocation {
        self.offset_to_location_in(&self.text, offset)
    }

    pub(super) fn offset_to_location_in(&self, text: &str, offset: usize) -> TextLocation {
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

    pub(super) fn selection_offsets(&self) -> (usize, usize) {
        let (start, end) = self.selection.ordered();
        (self.location_to_offset(start), self.location_to_offset(end))
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        if self.selection.is_empty() {
            return None;
        }
        let (start, end) = self.selection_offsets();
        self.text.get(start..end).map(ToString::to_string)
    }

    pub(super) fn selected_whole_rows(&self) -> Option<Range<usize>> {
        let line_lens: Vec<usize> = (0..self.render_line_count())
            .map(|row| self.line_len(row))
            .collect();
        whole_row_selection_range(self.selection, &line_lens)
    }
}
