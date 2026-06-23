use super::super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn move_cursor_to(
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

    pub(in crate::scheme_editor) fn move_left(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (start, _) = self.selection.ordered();
            self.move_cursor_to(start, false, cx);
            return;
        }
        if !select {
            if let Some(target) = self.table_boundary_horizontal_target(false) {
                self.move_cursor_to(target, false, cx);
                return;
            }
        }
        let offset = self.location_to_offset(self.selection.head);
        let prev = previous_char_boundary(&self.text, offset);
        let target = self.clamp_horizontal(self.selection.head, self.offset_to_location(prev));
        self.move_cursor_to(target, select, cx);
    }

    pub(in crate::scheme_editor) fn move_right(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (_, end) = self.selection.ordered();
            self.move_cursor_to(end, false, cx);
            return;
        }
        if !select {
            if let Some(target) = self.table_boundary_horizontal_target(true) {
                self.move_cursor_to(target, false, cx);
                return;
            }
            if self.insert_trailing_line_after_table_boundary(cx) {
                return;
            }
        }
        let offset = self.location_to_offset(self.selection.head);
        let next = next_char_boundary(&self.text, offset);
        let target = self.clamp_horizontal(self.selection.head, self.offset_to_location(next));
        self.move_cursor_to(target, select, cx);
    }

    pub(in crate::scheme_editor) fn move_vertical(&mut self, delta: isize, select: bool, cx: &mut Context<Self>) {
        if self.line_map.line_count() == 0 {
            let row_count = self.render_line_count();
            let row = (self.selection.head.row as isize + delta)
                .clamp(0, row_count.saturating_sub(1) as isize) as usize;
            let col = self.selection.head.col.min(self.line_len(row));
            self.move_cursor_to(TextLocation { row, col }, select, cx);
            return;
        }

        let target = self.vertical_target(self.selection.head, delta);
        self.move_cursor_to(target, select, cx);
    }

    pub(in crate::scheme_editor) fn move_word_left(&mut self, select: bool, cx: &mut Context<Self>) {
        let offset = self.location_to_offset(self.selection.head);
        let target = self.clamp_horizontal(
            self.selection.head,
            self.offset_to_location(previous_word_offset(&self.text, offset)),
        );
        self.move_cursor_to(target, select, cx);
    }

    pub(in crate::scheme_editor) fn move_word_right(&mut self, select: bool, cx: &mut Context<Self>) {
        let offset = self.location_to_offset(self.selection.head);
        let target = self.clamp_horizontal(
            self.selection.head,
            self.offset_to_location(next_word_offset(&self.text, offset)),
        );
        self.move_cursor_to(target, select, cx);
    }

    pub(in crate::scheme_editor) fn move_line_start(&mut self, select: bool, cx: &mut Context<Self>) {
        self.move_cursor_to(
            TextLocation {
                row: self.selection.head.row,
                col: 0,
            },
            select,
            cx,
        );
    }

    pub(in crate::scheme_editor) fn move_line_end(&mut self, select: bool, cx: &mut Context<Self>) {
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

    pub(in crate::scheme_editor) fn move_document_start(&mut self, select: bool, cx: &mut Context<Self>) {
        if let Some((first, _)) = self.cell_line_span(self.selection.head.row) {
            self.move_cursor_to(TextLocation { row: first, col: 0 }, select, cx);
            return;
        }
        self.move_cursor_to(TextLocation { row: 0, col: 0 }, select, cx);
    }

    pub(in crate::scheme_editor) fn move_document_end(&mut self, select: bool, cx: &mut Context<Self>) {
        if let Some((_, last)) = self.cell_line_span(self.selection.head.row) {
            self.move_cursor_to(
                TextLocation {
                    row: last,
                    col: self.line_len(last),
                },
                select,
                cx,
            );
            return;
        }
        self.move_cursor_to(self.offset_to_location(self.text.len()), select, cx);
    }

    pub(in crate::scheme_editor) fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some((first, last)) = self.cell_line_span(self.selection.head.row) {
            self.selection = TextSelection {
                anchor: TextLocation { row: first, col: 0 },
                head: TextLocation {
                    row: last,
                    col: self.line_len(last),
                },
            };
        } else {
            self.selection = TextSelection {
                anchor: TextLocation { row: 0, col: 0 },
                head: self.offset_to_location(self.text.len()),
            };
        }
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(in crate::scheme_editor) fn clamp_horizontal(&self, from: TextLocation, to: TextLocation) -> TextLocation {
        if to.row == from.row {
            return to;
        }
        let from_path = self
            .rows
            .get(from.row)
            .map(|row| row.path)
            .unwrap_or_default();
        let to_path = self
            .rows
            .get(to.row)
            .map(|row| row.path)
            .unwrap_or_default();
        let allowed = if from_path.is_cell() {
            to_path.is_cell()
                && to_path.anchor == from_path.anchor
                && to_path.r == from_path.r
                && to_path.c == from_path.c
        } else {
            to_path.is_doc()
        };
        if allowed {
            to
        } else if to.row < from.row {
            TextLocation {
                row: from.row,
                col: 0,
            }
        } else {
            TextLocation {
                row: from.row,
                col: self.line_len(from.row),
            }
        }
    }
}
