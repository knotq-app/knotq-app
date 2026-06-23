use super::super::*;

use super::previous_char_boundary_at;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn copy_selection_to_clipboard(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let text = self.selected_text()?;
        let whole_rows = self.selected_whole_rows();
        if let Some(rows) = whole_rows.clone() {
            let items = rows
                .clone()
                .filter_map(|row| self.rows.get(row).map(|row| row.item.clone()))
                .collect::<Vec<_>>();
            if !items.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                    text,
                    SchemeClipboardPayload::new(items),
                ));
                return Some(rows);
            }
        }
        if let Some(items) = self.selected_block_object_clipboard_items(&text) {
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                text,
                SchemeClipboardPayload::new_object_selection(items),
            ));
            return None;
        }
        // A selection that spans text *and* a block (image/table) but isn't a
        // clean whole-row range: capture the selected line fragments plus the
        // whole-line blocks as items, so the block survives the round-trip
        // instead of being dropped to a text-only copy.
        if let Some(items) = self.selected_block_line_items() {
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                text,
                SchemeClipboardPayload::new_spliced(items),
            ));
            return None;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        None
    }

    /// Items for a selection that includes at least one whole-line block: each
    /// selected text line contributes its selected substring, and each block its
    /// whole-line content. Returns `None` when no block is in the selection (so a
    /// pure-text selection keeps the inline-text clipboard behavior) or when the
    /// selection dips into a table cell.
    fn selected_block_line_items(&self) -> Option<Vec<Item>> {
        let (start, end) = self.selection.ordered();
        let mut items = Vec::new();
        let mut saw_block = false;
        for row in start.row..=end.row {
            let Some(editor_row) = self.rows.get(row) else {
                continue;
            };
            if editor_row.path.is_cell() {
                return None;
            }
            let line_len = self.line_len(row);
            let sel_start = (if row == start.row { start.col } else { 0 }).min(line_len);
            let sel_end = (if row == end.row { end.col } else { line_len }).min(line_len);
            if sel_start >= sel_end {
                continue;
            }
            let mut item = editor_row.item.clone();
            item.id = knotq_model::ItemId::new();
            if item_has_block_object(&editor_row.item) {
                saw_block = true;
            } else {
                let text = editor_row.item.text();
                let from = previous_char_boundary_at(&text, sel_start);
                let to = previous_char_boundary_at(&text, sel_end);
                item.set_text(text[from..to].to_string());
            }
            items.push(item);
        }

        (saw_block && !items.is_empty()).then_some(items)
    }

    fn selected_block_object_clipboard_items(&self, selected_text: &str) -> Option<Vec<Item>> {
        if selected_text.chars().any(|ch| ch != '\n') {
            return None;
        }

        let (start, end) = self.selection.ordered();
        let mut items = Vec::new();
        for row in start.row..=end.row {
            let Some(editor_row) = self.rows.get(row) else {
                continue;
            };
            if editor_row.path.is_cell() || !item_has_block_object(&editor_row.item) {
                continue;
            }
            let line_len = self.line_len(row);
            let selection_start = (if row == start.row { start.col } else { 0 }).min(line_len);
            let selection_end = (if row == end.row { end.col } else { line_len }).min(line_len);
            if selection_start >= selection_end {
                continue;
            }
            let Some(line) = self.line_range(row).and_then(|range| self.text.get(range)) else {
                continue;
            };
            for block in
                selected_block_inlines(&editor_row.item, line, selection_start..selection_end)
            {
                let mut item = Item::new("");
                item.indent = editor_row.item.indent;
                item.content = ItemContent::from_inlines(vec![block]);
                items.push(item);
            }
        }

        (!items.is_empty()).then_some(items)
    }

    pub(in crate::scheme_editor) fn copy(&mut self, cx: &mut Context<Self>) {
        self.copy_selection_to_clipboard(cx);
    }

    pub(in crate::scheme_editor) fn cut(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.read_only {
            self.copy_selection_to_clipboard(cx);
            return;
        }
        let Some(whole_rows) = self.copy_selection_to_clipboard(cx) else {
            if !self.selection.is_empty() {
                self.replace_selection("", window, cx);
            }
            return;
        };
        self.delete_whole_rows(whole_rows, window, cx);
    }

    pub(in crate::scheme_editor) fn delete_whole_rows(
        &mut self,
        rows: Range<usize>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        if rows.is_empty() || rows.start >= self.rows.len() {
            return;
        }

        let end = rows.end.min(self.rows.len());
        if rows.start >= end {
            return;
        }

        let mut items: Vec<Item> = self.rows.iter().map(|row| row.item.clone()).collect();
        let mut commands = Vec::new();
        for item in items[rows.start..end].iter() {
            commands.push(Command::DeleteItem {
                scheme: self.scheme_id,
                item: item.id,
            });
        }
        items.drain(rows.start..end);
        if items.is_empty() {
            let item = Item::new("");
            commands.push(Command::InsertItem {
                scheme: self.scheme_id,
                position: 0,
                item: item.clone(),
            });
            items.push(item);
        }

        let (text, editor_rows) = build_buffer(&items);
        self.text = text;
        self.rows = editor_rows;
        self.refresh_layout_after_content_change(window);
        let row = rows.start.min(self.rows.len().saturating_sub(1));
        self.selection = TextSelection::collapsed(TextLocation {
            row,
            col: self.line_len(row),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        self.emit_commands(commands, cx);
    }
}
