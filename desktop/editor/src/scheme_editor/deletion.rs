use super::*;

mod blocks;
mod boundary;
mod support;

use support::*;

impl SchemeEditor {
    /// Whether a deletion that touches a line/block boundary may proceed: the
    /// editor is writable and the caret is collapsed (no active selection).
    fn caret_delete_allowed(&self) -> bool {
        !self.read_only && self.selection.is_empty()
    }

    /// The block-aware guards every delete entry point runs first. Each consumes
    /// the keystroke when it applies (merging an adjacent block, removing a block
    /// at the caret or boundary, or refusing a delete that would eat a block), in
    /// which case the caller must return. Returns `true` if the keystroke was
    /// handled.
    fn block_aware_delete_guards(
        &mut self,
        prefer_backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.merge_adjacent_block_if_boundary(prefer_backward, window, cx)
            || self.delete_block_object_at_caret(prefer_backward, window, cx)
            || self.delete_adjacent_block_item_at_boundary(prefer_backward, window, cx)
            || self.boundary_delete_blocked(prefer_backward)
    }

    /// Rebuild the flat buffer from an edited top-level item list, refresh layout,
    /// place the caret (computed against the rebuilt rows by `select`), and emit
    /// the resulting CRDT diff. Shared by every block-level edit so the
    /// post-mutation bookkeeping stays identical.
    fn apply_top_level_edit(
        &mut self,
        old_top: &[Item],
        new_top: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<Self>,
        select: impl FnOnce(&Self) -> TextLocation,
    ) {
        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        let location = select(self);
        self.selection = TextSelection::collapsed(location);
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();
        self.emit_top_level_diff(old_top, &new_top, cx);
    }

    /// Like [`Self::apply_top_level_edit`] but for an edit that *removes* a single
    /// top-level item: it emits a `DeleteItem` command (instead of a structural
    /// diff) and accepts an optional window so it can run during shutdown saves.
    fn apply_top_level_item_delete(
        &mut self,
        new_top: Vec<Item>,
        deleted: ItemId,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
        select: impl FnOnce(&Self) -> TextLocation,
    ) {
        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        let location = select(self);
        self.selection = TextSelection::collapsed(location);
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();
        cx.emit(EditorEvent::Command(Command::DeleteItem {
            scheme: self.scheme_id,
            item: deleted,
        }));
        cx.notify();
    }

    pub(super) fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.selection.is_empty() {
            if let Some(path) = self.rows.get(self.selection.head.row).map(|row| row.path) {
                if path.is_header_cell() {
                    if let Some(row) = self.find_cell_row(path.anchor, 0, path.c, false) {
                        self.move_cursor_to(TextLocation { row, col: 0 }, false, cx);
                    }
                    return;
                }
                if !path.is_cell() && self.handle_block_object_enter(window, cx) {
                    return;
                }
            }
        }
        self.replace_selection("\n", Some(window), cx);
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
        if self.delete_selected_block_object(window, cx) {
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
        if self.block_aware_delete_guards(true, window, cx) {
            return;
        }
        // Check auto-bulletize undo before anything else.
        if let Some((undo_row, original_text, original_marker)) = self.auto_bullet_undo.take() {
            if self.selection.is_empty() && self.selection.head.row == undo_row {
                // Revert: restore original text and marker.
                if let Some(editor_row) = self.rows.get_mut(undo_row) {
                    editor_row.item.set_text(original_text.clone());
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
        if self.block_aware_delete_guards(true, window, cx) {
            return;
        }
        if self.delete_preflight(true, window, cx) {
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        let prev = previous_word_offset(&self.text, offset);
        self.replace_byte_range(prev..offset, "", Some(window), cx);
    }

    pub(super) fn backspace_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_aware_delete_guards(true, window, cx) {
            return;
        }
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
        if self.block_aware_delete_guards(false, window, cx) {
            return;
        }
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
        if self.block_aware_delete_guards(false, window, cx) {
            return;
        }
        if self.delete_preflight(false, window, cx) {
            return;
        }
        let offset = self.location_to_offset(self.selection.head);
        let next = next_word_offset(&self.text, offset);
        self.replace_byte_range(offset..next, "", Some(window), cx);
    }

    pub(super) fn delete_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.block_aware_delete_guards(false, window, cx) {
            return;
        }
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
