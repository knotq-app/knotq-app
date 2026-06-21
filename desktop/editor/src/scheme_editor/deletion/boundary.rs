use super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn boundary_delete_blocked(&self, backward: bool) -> bool {
        if !self.selection.is_empty() {
            return false;
        }
        let head = self.selection.head;
        let Some(current) = self.rows.get(head.row) else {
            return false;
        };
        let path = current.path;
        // A whole-line block (image/table) is atomic. A collapsed-cursor delete at
        // its edge is handled earlier by `delete_block_object_at_caret`, which
        // removes the block and leaves an empty text line; what stays blocked here
        // is merging an adjacent *text* line into a block, which would silently eat
        // that text. (`!is_cell` keeps this to document-level blocks; inside a
        // table cell, editing is ordinary.)
        let current_is_block = !path.is_cell() && item_has_block_object(&current.item);

        if backward {
            if head.col != 0 {
                // The only other caret position on a block line is after the
                // object; that backspace is handled by the caret deletion above.
                return current_is_block;
            }
            if head.row == 0 {
                return false;
            }
            // Deleting an *empty* line that sits against a block just removes the
            // empty line — allowed, handled by the empty-line boundary path.
            if self.empty_doc_line_adjacent_to_block(head.row) {
                return false;
            }
            if current_is_block {
                return true;
            }
            let previous = &self.rows[head.row - 1];
            if !previous.path.is_cell() && item_has_block_object(&previous.item) {
                return true;
            }
            !same_region(previous.path, path)
        } else {
            if head.col != self.line_len(head.row) {
                return current_is_block;
            }
            if head.row + 1 >= self.rows.len() {
                return false;
            }
            if self.empty_doc_line_adjacent_to_block(head.row) {
                return false;
            }
            if current_is_block {
                return true;
            }
            let next = &self.rows[head.row + 1];
            if !next.path.is_cell() && item_has_block_object(&next.item) {
                return true;
            }
            !same_region(next.path, path)
        }
    }

    fn empty_doc_line_adjacent_to_block(&self, row: usize) -> bool {
        is_empty_doc_line_adjacent_to_block(&self.rows, row, self.line_len(row))
    }

    pub(in crate::scheme_editor) fn clear_current_line_attributes_if_empty(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.caret_delete_allowed() {
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

    pub(in crate::scheme_editor) fn clear_current_line_attributes_if_boundary_delete(
        &mut self,
        prefer_backward: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.caret_delete_allowed() {
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
            // A backspace at the very start of a line strips the line's marker
            // before it would merge upward. On the first line there is no line to
            // merge into (offset == 0), but the marker should still clear — that
            // is the only way to remove the marker from the first line.
            col == 0
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

    pub(in crate::scheme_editor) fn delete_empty_line_boundary_if_possible(
        &mut self,
        prefer_backward: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.caret_delete_allowed() {
            return false;
        }

        if rows_have_block_object(&self.rows) {
            return self.delete_empty_doc_line_adjacent_to_block(prefer_backward, window, cx);
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

        self.apply_top_level_item_delete(items, deleted, window, cx, |this| {
            this.clamp_location(plan.cursor_after)
        });
        true
    }

    fn delete_empty_doc_line_adjacent_to_block(
        &mut self,
        prefer_backward: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        let row = self.current_row_index();
        if !self.empty_doc_line_adjacent_to_block(row) {
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
        let deleted_last_top = top_index >= new_top.len();

        self.apply_top_level_item_delete(new_top, deleted, window, cx, |this| {
            let target_top = if prefer_backward || deleted_last_top {
                top_index.saturating_sub(1)
            } else {
                top_index
            };
            let target_row = flat_row_for_top_level_index(&this.rows, target_top);
            let target_col = if prefer_backward || deleted_last_top {
                this.line_len(target_row)
            } else {
                0
            };
            this.clamp_location(TextLocation {
                row: target_row,
                col: target_col,
            })
        });
        true
    }
}
