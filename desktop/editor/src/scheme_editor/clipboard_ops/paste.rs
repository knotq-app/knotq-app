use super::super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn paste_text(
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

    pub(in crate::scheme_editor) fn paste_plain(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
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

    pub(in crate::scheme_editor) fn paste_plain_text_as_items(
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

    pub(in crate::scheme_editor) fn paste(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
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
            if payload.object_selection {
                self.paste_rich_objects(payload.items, window, cx);
                return;
            }
            if payload.splice {
                self.paste_spliced_items(payload.items, window, cx);
                return;
            }
            self.paste_rich_items(payload.items, window, cx);
            return;
        }
        if let Some(text) = item.text() {
            self.paste_text(&text, window, cx);
        }
    }


    pub(in crate::scheme_editor) fn paste_spliced_items(
        &mut self,
        items: Vec<Item>,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || items.is_empty() {
            return false;
        }
        // Replace any active selection first, then splice at the resulting caret.
        if !self.selection.is_empty() {
            self.replace_selection("", window.as_deref_mut(), cx);
        }
        let row = self
            .current_row_index()
            .min(self.rows.len().saturating_sub(1));
        let Some(editor_row) = self.rows.get(row) else {
            return false;
        };
        // A table cell or block line has no text position to splice into — fall
        // back to inserting the items as their own lines.
        if editor_row.path.is_cell() || item_has_block_object(&editor_row.item) {
            return self.paste_rich_items(items, window, cx);
        }
        let col = self.selection.head.col.min(self.line_len(row));

        let old_top = reconstruct_top_level(&self.rows);
        let Some(pos) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };
        let Some(current) = old_top.get(pos) else {
            return false;
        };
        // Re-id so the pasted items stay distinct from any originals still present.
        let mut items = items;
        for item in &mut items {
            item.id = knotq_model::ItemId::new();
        }
        let (replacement, cursor_index, cursor_col) = splice_items_into_line(current, col, items);
        let inserted = replacement.len();

        let mut new_top = old_top.clone();
        new_top.splice(pos..=pos, replacement);

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        let cursor_top = pos + cursor_index.min(inserted.saturating_sub(1));
        let target_row = flat_row_for_top_level_index(&self.rows, cursor_top);
        self.selection = TextSelection::collapsed(TextLocation {
            row: target_row,
            col: cursor_col.min(self.line_len(target_row)),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        self.emit_top_level_diff(&old_top, &new_top, cx);
        cx.notify();
        true
    }

    pub(in crate::scheme_editor) fn paste_rich_items(
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

    pub(in crate::scheme_editor) fn paste_rich_objects(
        &mut self,
        items: Vec<Item>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let blocks = items
            .into_iter()
            .flat_map(|item| item.content.to_inlines())
            .filter(|inline| !inline.is_text())
            .collect::<Vec<_>>();
        if blocks.is_empty() {
            return false;
        }

        let (start, end) = self.selection.ordered();
        if start.row != end.row {
            return false;
        }
        let row = start.row.min(self.rows.len().saturating_sub(1));
        let Some(editor_row) = self.rows.get(row) else {
            return false;
        };
        if editor_row.path.is_cell() {
            return false;
        }
        // Pasting block objects at a caret (no selection) on a normal line splits
        // the line so leading/trailing text survive as their own lines.
        if start.col == end.col {
            let col = start.col.min(self.line_len(row));
            return self.insert_block_lines_at_doc_row(row, col, blocks, window, cx);
        }
        let Some(line) = self
            .line_range(row)
            .and_then(|range| self.text.get(range))
            .map(ToOwned::to_owned)
        else {
            return false;
        };

        let range = start.col.min(line.len())..end.col.min(line.len());
        let cursor_col = range.start + blocks.len() * TABLE_OBJECT_LEN;
        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let Some(pos) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };
        let Some(item) = new_top.get_mut(pos) else {
            return false;
        };
        if !replace_block_range_with_inlines(item, &line, range, blocks) {
            return false;
        }

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        let row = flat_row_for_top_level_index(&self.rows, pos);
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

    pub(in crate::scheme_editor) fn replace_rows_with_items(
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

    pub(in crate::scheme_editor) fn rich_paste_delete_range(&self) -> Range<usize> {
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
