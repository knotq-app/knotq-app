use super::*;

impl SchemeEditor {
    pub(super) fn copy_selection_to_clipboard(
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
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        None
    }

    pub(super) fn copy(&mut self, cx: &mut Context<Self>) {
        self.copy_selection_to_clipboard(cx);
    }

    pub(super) fn cut(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
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

    pub(super) fn delete_whole_rows(
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

    pub(super) fn paste_text(
        &mut self,
        text: &str,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        self.replace_selection(text, window, cx);
    }

    pub(super) fn paste_plain(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            if let Some(rows) = self.selected_whole_rows() {
                self.paste_plain_text_as_items(rows, &text, window, cx);
                return;
            }
            if self.selection.is_empty() {
                let row = self
                    .current_row_index()
                    .min(self.rows.len().saturating_sub(1));
                if self.line_len(row) == 0 {
                    self.paste_plain_text_as_items(row..row + 1, &text, window, cx);
                    return;
                }
            }
            let (start, end) = self.selection_offsets();
            self.replace_byte_range_with_style_inference(start..end, &text, false, window, cx);
        }
    }

    pub(super) fn paste_plain_text_as_items(
        &mut self,
        delete_range: Range<usize>,
        text: &str,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let items = text
            .split('\n')
            .map(|line| Item::new(clean_line_text(line)))
            .collect::<Vec<_>>();
        self.replace_rows_with_items(delete_range, items, window, cx)
    }

    pub(super) fn paste(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(image) = clipboard_image(&item) {
            self.paste_image(image, window, cx);
            return;
        }
        if let Some(payload) = rich_clipboard_payload(&item) {
            self.paste_rich_items(payload.items, window, cx);
            return;
        }
        if let Some(text) = item.text() {
            self.paste_text(&text, window, cx);
        }
    }

    pub(super) fn paste_image(
        &mut self,
        image: &Image,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let Some(media) = persist_clipboard_image(image) else {
            return false;
        };
        let mut item = Item::new("");
        item.media.push(media);
        self.replace_rows_with_items(self.rich_paste_delete_range(), vec![item], window, cx)
    }

    pub(super) fn paste_rich_items(
        &mut self,
        items: Vec<Item>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        if items.is_empty() {
            return false;
        }
        if !self.selection.is_empty() && self.selected_whole_rows().is_none() {
            return false;
        }

        let pasted_items = items
            .into_iter()
            .map(item_for_rich_paste)
            .collect::<Vec<_>>();
        let delete_range = self.rich_paste_delete_range();
        self.replace_rows_with_items(delete_range, pasted_items, window, cx)
    }

    pub(super) fn replace_rows_with_items(
        &mut self,
        delete_range: Range<usize>,
        inserted_items: Vec<Item>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        if inserted_items.is_empty() {
            return false;
        }

        let mut current_items: Vec<Item> = self.rows.iter().map(|row| row.item.clone()).collect();
        let delete_start = delete_range.start.min(current_items.len());
        let delete_end = delete_range.end.min(current_items.len());
        let insert_at = delete_start.min(current_items.len());

        let mut commands = Vec::new();
        for item in current_items[delete_start..delete_end].iter() {
            commands.push(Command::DeleteItem {
                scheme: self.scheme_id,
                item: item.id,
            });
        }
        current_items.drain(delete_start..delete_end);
        for (offset, item) in inserted_items.iter().cloned().enumerate() {
            let position = insert_at + offset;
            current_items.insert(position, item.clone());
            commands.push(Command::InsertItem {
                scheme: self.scheme_id,
                position,
                item,
            });
        }

        let (text, rows) = build_buffer(&current_items);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        let cursor_row = insert_at + inserted_items.len().saturating_sub(1);
        self.selection = TextSelection::collapsed(TextLocation {
            row: cursor_row.min(self.rows.len().saturating_sub(1)),
            col: self.line_len(cursor_row),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        self.emit_commands(commands, cx);
        true
    }

    pub(super) fn rich_paste_delete_range(&self) -> Range<usize> {
        if let Some(rows) = self.selected_whole_rows() {
            return rows;
        }

        let row = self
            .current_row_index()
            .min(self.rows.len().saturating_sub(1));
        let col = self.selection.head.col.min(self.line_len(row));
        if self.line_len(row) == 0 {
            return row..row + 1;
        }
        if col == 0 {
            row..row
        } else {
            row + 1..row + 1
        }
    }
}
