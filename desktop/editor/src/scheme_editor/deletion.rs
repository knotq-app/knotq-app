use super::*;

impl SchemeEditor {
    pub(super) fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.replace_selection("\n", Some(window), cx);
    }

    pub(super) fn clear_current_line_attributes_if_empty(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        if !self.selection.is_empty() {
            return false;
        }

        let row = self
            .selection
            .head
            .row
            .min(self.render_line_count().saturating_sub(1));
        if self.line_len(row) != 0 {
            return false;
        }

        let Some(item) = self.rows.get(row).map(|row| row.item.clone()) else {
            return false;
        };
        if item.marker == ItemMarker::Blank {
            return false;
        }
        if !item_has_line_attributes(&item) {
            return false;
        }

        let clean_item = item_without_line_attributes(&item);
        if let Some(row) = self.rows.get_mut(row) {
            row.item = clean_item.clone();
        }
        cx.emit(EditorEvent::Command(Command::ReplaceItem {
            scheme: self.scheme_id,
            item: clean_item,
        }));
        cx.emit(EditorEvent::CloseDatePopover);
        self.reset_cursor_blink(cx);
        cx.notify();
        true
    }

    pub(super) fn clear_current_line_attributes_if_boundary_delete(
        &mut self,
        prefer_backward: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        if !self.selection.is_empty() {
            return false;
        }

        let row = self
            .selection
            .head
            .row
            .min(self.render_line_count().saturating_sub(1));
        let col = self.selection.head.col.min(self.line_len(row));
        let offset = self.location_to_offset(self.selection.head);
        let would_delete_line_boundary = if prefer_backward {
            col == 0 && offset > 0
        } else {
            col == self.line_len(row) && offset < self.text.len()
        };
        if !would_delete_line_boundary {
            return false;
        }

        let Some(item) = self.rows.get(row).map(|row| row.item.clone()) else {
            return false;
        };
        if item.marker == ItemMarker::Blank || !item_has_line_attributes(&item) {
            return false;
        }

        let clean_item = item_without_line_attributes(&item);
        if let Some(row) = self.rows.get_mut(row) {
            row.item = clean_item.clone();
        }
        cx.emit(EditorEvent::Command(Command::ReplaceItem {
            scheme: self.scheme_id,
            item: clean_item,
        }));
        cx.emit(EditorEvent::CloseDatePopover);
        self.reset_cursor_blink(cx);
        cx.notify();
        true
    }

    pub(super) fn delete_empty_line_boundary_if_possible(
        &mut self,
        prefer_backward: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        if !self.selection.is_empty() {
            return false;
        }

        let row = self.current_row_index();
        if self.line_len(row) != 0 {
            return false;
        }

        let row_count = self.rows.len();
        let row = row.min(row_count.saturating_sub(1));
        let previous_line_len = row
            .checked_sub(1)
            .map(|previous| self.line_len(previous))
            .unwrap_or(0);
        let Some(plan) = empty_line_delete_plan(row, row_count, prefer_backward, previous_line_len)
        else {
            return false;
        };

        let mut items: Vec<Item> = self.rows.iter().map(|row| row.item.clone()).collect();
        let Some(deleted) = items.get(plan.delete_row).map(|item| item.id) else {
            return false;
        };
        items.remove(plan.delete_row);

        let (text, rows) = build_buffer(&items);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        self.selection = TextSelection::collapsed(self.clamp_location(plan.cursor_after));
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();
        cx.emit(EditorEvent::Command(Command::DeleteItem {
            scheme: self.scheme_id,
            item: deleted,
        }));
        cx.notify();
        true
    }

    pub(super) fn delete_preflight(
        &mut self,
        prefer_backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return true;
        }
        if !self.selection.is_empty() {
            self.replace_selection("", Some(window), cx);
            return true;
        }
        self.clear_current_line_attributes_if_empty(cx)
            || self.clear_current_line_attributes_if_boundary_delete(prefer_backward, cx)
            || self.delete_empty_line_boundary_if_possible(prefer_backward, Some(window), cx)
    }

    pub(super) fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Check auto-bulletize undo before anything else.
        if let Some((undo_row, original_text, original_marker)) = self.auto_bullet_undo.take() {
            if self.selection.is_empty() && self.selection.head.row == undo_row {
                // Revert: restore original text and marker.
                if let Some(editor_row) = self.rows.get_mut(undo_row) {
                    editor_row.item.text = original_text.clone();
                    editor_row.item.marker = original_marker;

                    let items: Vec<Item> = self.rows.iter().map(|r| r.item.clone()).collect();
                    let (text, rows) = build_buffer(&items);
                    self.text = text;
                    self.rows = rows;
                    let col = original_text.len();
                    self.selection = TextSelection::collapsed(TextLocation { row: undo_row, col });
                    self.marked_range = None;
                    self.refresh_layout_after_content_change(Some(window));
                    self.reset_cursor_blink(cx);
                    self.scroll_to_cursor(cx);

                    let item = self.rows[undo_row].item.clone();
                    cx.emit(EditorEvent::Command(Command::ReplaceItem {
                        scheme: self.scheme_id,
                        item,
                    }));
                    cx.notify();
                    return;
                }
            }
            // If we're on a different row, discard the undo and proceed normally.
        }

        if self.delete_preflight(true, window, cx) {
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        if offset == 0 {
            return;
        }
        let prev = previous_char_boundary(&self.text, offset);
        self.replace_byte_range(prev..offset, "", Some(window), cx);
    }

    pub(super) fn backspace_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_preflight(true, window, cx) {
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        let prev = previous_word_offset(&self.text, offset);
        self.replace_byte_range(prev..offset, "", Some(window), cx);
    }

    pub(super) fn backspace_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_preflight(true, window, cx) {
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        let line_start = self
            .line_range(self.selection.head.row)
            .map(|range| range.start)
            .unwrap_or(0);
        if line_start == offset && line_start > 0 {
            self.replace_byte_range(line_start - 1..offset, "", Some(window), cx);
        } else {
            self.replace_byte_range(line_start..offset, "", Some(window), cx);
        }
    }

    pub(super) fn delete_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_preflight(false, window, cx) {
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        if offset >= self.text.len() {
            return;
        }
        let next = next_char_boundary(&self.text, offset);
        self.replace_byte_range(offset..next, "", Some(window), cx);
    }

    pub(super) fn delete_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_preflight(false, window, cx) {
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        let next = next_word_offset(&self.text, offset);
        self.replace_byte_range(offset..next, "", Some(window), cx);
    }

    pub(super) fn delete_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_preflight(false, window, cx) {
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        let line_end = self
            .line_range(self.selection.head.row)
            .map(|range| range.end)
            .unwrap_or(self.text.len());
        if line_end == offset && line_end < self.text.len() {
            self.replace_byte_range(offset..line_end + 1, "", Some(window), cx);
        } else {
            self.replace_byte_range(offset..line_end, "", Some(window), cx);
        }
    }
}
