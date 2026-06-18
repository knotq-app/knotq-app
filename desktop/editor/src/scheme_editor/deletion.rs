use super::*;

impl SchemeEditor {
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
                if path.is_table_anchor() && self.handle_table_anchor_enter(window, cx) {
                    return;
                }
            }
        }
        self.replace_selection("\n", Some(window), cx);
    }

    fn handle_table_anchor_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.selection.is_empty() {
            return false;
        }
        let row = self.current_row_index();
        let Some(editor_row) = self.rows.get(row) else {
            return false;
        };
        if !editor_row.path.is_table_anchor() {
            return false;
        }
        let Some(top_index) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };

        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let mut col = self.selection.head.col.min(self.line_len(row));
        if let Some(object) = self.table_object_range_for_row(row) {
            if col > object.start {
                return self.insert_blank_after_table_anchor(row, window, cx);
            }
            col = col.min(object.start);
        }
        let Some(result) = split_table_item_at_text_col(&mut new_top, top_index, col) else {
            return false;
        };

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        let row = self
            .rows
            .iter()
            .position(|row| row.path.is_table_anchor() && row.item.id == result.table)
            .unwrap_or_else(|| flat_row_for_top_level_index(&self.rows, result.table_index));
        self.selection = TextSelection::collapsed(TextLocation { row, col: 0 });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();
        self.emit_top_level_diff(&old_top, &new_top, cx);
        true
    }

    fn insert_blank_after_table_anchor(
        &mut self,
        anchor_row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(anchor) = self.rows.get(anchor_row) else {
            return false;
        };
        let table_id = anchor.item.id;
        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let Some(pos) = new_top.iter().position(|item| item.id == table_id) else {
            return false;
        };

        let mut blank = Item::new("");
        blank.indent = anchor.item.indent;
        new_top.insert(pos + 1, blank);

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        let row = flat_row_for_top_level_index(&self.rows, pos + 1);
        self.selection = TextSelection::collapsed(TextLocation { row, col: 0 });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();
        self.emit_top_level_diff(&old_top, &new_top, cx);
        true
    }

    fn boundary_delete_blocked(&self, backward: bool) -> bool {
        if !self.selection.is_empty() {
            return false;
        }
        let head = self.selection.head;
        let path = self
            .rows
            .get(head.row)
            .map(|row| row.path)
            .unwrap_or_default();

        if backward {
            if head.col != 0 || head.row == 0 {
                return false;
            }
            if self.empty_doc_line_adjacent_to_table(head.row) {
                return false;
            }
            let previous = self.rows[head.row - 1].path;
            !same_region(previous, path)
        } else {
            if head.col != self.line_len(head.row) || head.row + 1 >= self.rows.len() {
                return false;
            }
            if self.empty_doc_line_adjacent_to_table(head.row) {
                return false;
            }
            let next = self.rows[head.row + 1].path;
            !same_region(next, path)
        }
    }

    fn empty_doc_line_adjacent_to_table(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|editor_row| {
            editor_row.path.is_doc()
                && self.line_len(row) == 0
                && (row
                    .checked_sub(1)
                    .and_then(|previous| self.rows.get(previous))
                    .is_some_and(|previous| previous.path.is_table_anchor())
                    || self
                        .rows
                        .get(row + 1)
                        .is_some_and(|next| next.path.is_table_anchor()))
        })
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

        // Stripping the marker keeps the line's indentation.
        let mut clean_item = item_without_line_attributes(&item);
        clean_item.indent = item.indent;
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

        // Stripping the marker keeps the line's indentation.
        let mut clean_item = item_without_line_attributes(&item);
        clean_item.indent = item.indent;
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

        if rows_have_table(&self.rows) {
            return self.delete_empty_doc_line_adjacent_to_table(prefer_backward, window, cx);
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

    fn delete_empty_doc_line_adjacent_to_table(
        &mut self,
        prefer_backward: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        let row = self.current_row_index();
        if !self.empty_doc_line_adjacent_to_table(row) {
            return false;
        }
        let Some(top_index) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };

        let old_top = reconstruct_top_level(&self.rows);
        let Some(item) = old_top.get(top_index) else {
            return false;
        };
        if !item.is_content_empty() {
            return false;
        }
        let deleted = item.id;
        let mut new_top = old_top.clone();
        new_top.remove(top_index);

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        let target_top = if prefer_backward {
            top_index.saturating_sub(1)
        } else {
            top_index.min(new_top.len().saturating_sub(1))
        };
        let target_row = flat_row_for_top_level_index(&self.rows, target_top);
        let target_col = if prefer_backward {
            self.line_len(target_row)
        } else {
            0
        };
        self.selection = TextSelection::collapsed(self.clamp_location(TextLocation {
            row: target_row,
            col: target_col,
        }));
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
        if self.delete_selected_table_object(window, cx) {
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

    fn delete_selected_table_object(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        let (start, end) = self.selection.ordered();
        if start.row != end.row {
            return false;
        }
        let Some(object) = self.table_object_range_for_row(start.row) else {
            return false;
        };
        if start.col != object.start || end.col != object.end {
            return false;
        }
        self.delete_table_from_anchor_row(start.row, window, cx)
    }

    pub(super) fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.merge_adjacent_table_if_boundary(true, window, cx)
            || self.delete_table_at_backward_boundary(window, cx)
        {
            return;
        }
        if self.boundary_delete_blocked(true) {
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

    fn delete_table_at_backward_boundary(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.selection.is_empty() {
            return false;
        }
        let row = self.selection.head.row;
        let Some(editor_row) = self.rows.get(row) else {
            return false;
        };
        if !editor_row.path.is_table_anchor() {
            return false;
        }
        let Some(object) = self.table_object_range_for_row(row) else {
            return false;
        };
        if self.selection.head.col != object.end {
            return false;
        }
        self.delete_table_from_anchor_row(row, window, cx)
    }

    pub(super) fn backspace_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.merge_adjacent_table_if_boundary(true, window, cx)
            || self.delete_table_at_backward_boundary(window, cx)
        {
            return;
        }
        if self.boundary_delete_blocked(true) {
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
        if self.merge_adjacent_table_if_boundary(true, window, cx)
            || self.delete_table_at_backward_boundary(window, cx)
        {
            return;
        }
        if self.boundary_delete_blocked(true) {
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
        if self.delete_table_at_forward_boundary(window, cx)
            || self.merge_adjacent_table_if_boundary(false, window, cx)
        {
            return;
        }
        if self.boundary_delete_blocked(false) {
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
        if self.delete_table_at_forward_boundary(window, cx)
            || self.merge_adjacent_table_if_boundary(false, window, cx)
        {
            return;
        }
        if self.boundary_delete_blocked(false) {
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
        if self.delete_table_at_forward_boundary(window, cx)
            || self.merge_adjacent_table_if_boundary(false, window, cx)
        {
            return;
        }
        if self.boundary_delete_blocked(false) {
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

    fn delete_table_at_forward_boundary(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.selection.is_empty() {
            return false;
        }
        let row = self.current_row_index();
        let Some(editor_row) = self.rows.get(row) else {
            return false;
        };
        if editor_row.path.is_cell() {
            return false;
        }

        if editor_row.path.is_table_anchor() {
            let Some(object) = self.table_object_range_for_row(row) else {
                return false;
            };
            if self.selection.head.col <= object.start {
                return self.delete_table_from_anchor_row(row, window, cx);
            }
            return false;
        }

        if self.selection.head.col != self.line_len(row) {
            return false;
        }

        if self.line_len(row) == 0 {
            let next = row + 1;
            if self
                .rows
                .get(next)
                .is_some_and(|row| row.path.is_table_anchor())
            {
                return self.delete_table_item_after_row(row, next, window, cx);
            }
        }

        false
    }

    fn delete_table_from_anchor_row(
        &mut self,
        anchor_row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(anchor) = self.rows.get(anchor_row) else {
            return false;
        };
        let table_id = anchor.item.id;
        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let Some(pos) = new_top.iter().position(|item| item.id == table_id) else {
            return false;
        };
        let Some(table_pos) = new_top[pos]
            .content
            .iter()
            .position(|inline| matches!(inline, Inline::Table(_)))
        else {
            return false;
        };
        new_top[pos].content.remove(table_pos);

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        let row = flat_row_for_top_level_index(&self.rows, pos);
        self.selection = TextSelection::collapsed(TextLocation {
            row,
            col: self.line_len(row),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();
        self.emit_top_level_diff(&old_top, &new_top, cx);
        true
    }

    fn delete_table_item_after_row(
        &mut self,
        cursor_row: usize,
        table_row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(table_item) = self.rows.get(table_row) else {
            return false;
        };
        let table_id = table_item.item.id;
        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let Some(pos) = new_top.iter().position(|item| item.id == table_id) else {
            return false;
        };
        new_top.remove(pos);

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        let row = cursor_row.min(self.rows.len().saturating_sub(1));
        self.selection = TextSelection::collapsed(TextLocation {
            row,
            col: self.line_len(row),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();
        self.emit_top_level_diff(&old_top, &new_top, cx);
        true
    }

    fn merge_adjacent_table_if_boundary(
        &mut self,
        prefer_backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.selection.is_empty() {
            return false;
        };

        let row = self.current_row_index();
        let path = self.rows.get(row).map(|row| row.path).unwrap_or_default();
        if path.is_cell() {
            return false;
        }

        let col = self.selection.head.col.min(self.line_len(row));
        let Some((target_top, table_top, cursor_col)) = (if prefer_backward {
            if col != 0 || !path.is_table_anchor() {
                None
            } else {
                top_level_index_for_flat_row(&self.rows, row).and_then(|table_top| {
                    table_top.checked_sub(1).map(|target_top| {
                        let target_row = flat_row_for_top_level_index(&self.rows, target_top);
                        (target_top, table_top, self.line_len(target_row))
                    })
                })
            }
        } else if col != self.line_len(row) {
            None
        } else {
            top_level_index_for_flat_row(&self.rows, row).and_then(|target_top| {
                let table_top = target_top.checked_add(1)?;
                let table_row = flat_row_for_top_level_index(&self.rows, table_top);
                self.rows
                    .get(table_row)
                    .filter(|row| row.path.is_table_anchor())
                    .map(|_| (target_top, table_top, col))
            })
        }) else {
            return false;
        };

        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let Some(result) = merge_table_item_into(&mut new_top, target_top, table_top) else {
            return false;
        };

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        let row = flat_row_for_top_level_index(&self.rows, result.target_index);
        self.selection = TextSelection::collapsed(TextLocation {
            row,
            col: cursor_col.min(self.line_len(row)),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();
        self.emit_top_level_diff(&old_top, &new_top, cx);
        true
    }
}

fn same_region(a: RowPath, b: RowPath) -> bool {
    if a.is_cell() && b.is_cell() {
        a.anchor == b.anchor && a.r == b.r && a.c == b.c
    } else {
        a.is_doc() && b.is_doc()
    }
}
