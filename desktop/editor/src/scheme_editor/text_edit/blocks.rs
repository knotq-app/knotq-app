use super::super::*;

impl SchemeEditor {
    /// When a printable character is typed with the caret sitting right before or
    /// after a whole-line block (image/table), insert the text on a new adjacent
    /// line instead of letting the edit collapse into the block (a no-op).
    /// Returns `true` if it handled the input.
    pub(in crate::scheme_editor) fn try_type_adjacent_to_block(
        &mut self,
        offset: usize,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || text.is_empty() || text.contains('\n') {
            return false;
        }
        let loc = self.offset_to_location(offset);
        let Some(row) = self.rows.get(loc.row) else {
            return false;
        };
        // Only doc-level block lines — inside a table cell, text editing is normal.
        if row.path.is_cell() || !item_has_block_object(&row.item) {
            return false;
        }
        let before = loc.col == 0;
        self.insert_text_line_adjacent_to_block(loc.row, before, text, cx)
    }

    fn insert_text_line_adjacent_to_block(
        &mut self,
        row: usize,
        before: bool,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let old_top = reconstruct_top_level(&self.rows);
        let Some(pos) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };
        let mut new_top = old_top.clone();
        let indent = new_top.get(pos).map(|item| item.indent).unwrap_or(0);
        let mut item = Item::new(text);
        item.indent = indent;
        let insert_pos = if before { pos } else { pos + 1 };
        let insert_pos = insert_pos.min(new_top.len());
        new_top.insert(insert_pos, item);

        let (buffer_text, rows) = build_buffer(&new_top);
        self.text = buffer_text;
        self.rows = rows;
        self.refresh_layout_after_content_change(None);
        let target_row = flat_row_for_top_level_index(&self.rows, insert_pos);
        self.selection = TextSelection::collapsed(TextLocation {
            row: target_row,
            col: self.line_len(target_row),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        self.emit_top_level_diff(&old_top, &new_top, cx);
        cx.notify();
        true
    }
}
