use super::*;

impl SchemeEditor {
    pub(super) fn insert_image_from_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }

        self.focus(window, cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Insert image".into()),
        });
        cx.spawn(
            async move |editor: gpui::WeakEntity<SchemeEditor>, cx: &mut gpui::AsyncApp| {
                let paths = match paths.await {
                    Ok(Ok(Some(paths))) => paths,
                    _ => return,
                };
                let media = paths
                    .iter()
                    .filter_map(|path| persist_image_file(path))
                    .collect::<Vec<_>>();
                if media.is_empty() {
                    return;
                }
                let _ = editor.update(cx, |editor, cx| {
                    let row = editor.current_row_index();
                    editor.append_media_to_row(row, media, None, cx);
                });
            },
        )
        .detach();
    }

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
        let row = self.current_row_index();
        self.append_media_to_row(row, vec![media], window, cx)
    }

    pub(super) fn drop_image_paths(
        &mut self,
        paths: &ExternalPaths,
        position: Point<Pixels>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let media = paths
            .paths()
            .iter()
            .filter_map(|path| persist_image_file(path))
            .collect::<Vec<_>>();
        if media.is_empty() {
            return false;
        }
        let row = self
            .location_for_window_position(position)
            .row
            .min(self.rows.len().saturating_sub(1));
        self.selection = TextSelection::collapsed(TextLocation {
            row,
            col: self.line_len(row),
        });
        self.append_media_to_row(row, media, window, cx)
    }

    pub(super) fn append_media_to_row(
        &mut self,
        row: usize,
        media: Vec<ImageInline>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || media.is_empty() || self.rows.is_empty() {
            return false;
        }
        let row = row.min(self.rows.len().saturating_sub(1));
        let insert_col = if self.selection.head.row == row {
            self.selection.head.col.min(self.line_len(row))
        } else {
            self.line_len(row)
        };
        let is_cell = self
            .rows
            .get(row)
            .map(|r| r.path.is_cell())
            .unwrap_or(false);
        if !is_cell {
            // Inserting an image into a normal line splits it: leading/trailing
            // text stay as their own lines and the image lands on a line of its own.
            let blocks = media.into_iter().map(Inline::Image).collect::<Vec<_>>();
            return self.insert_block_lines_at_doc_row(row, insert_col, blocks, window, cx);
        }

        // Table cell line: insert in place (cells are sub-documents).
        let old_top = reconstruct_top_level(&self.rows);
        let Some(editor_row) = self.rows.get_mut(row) else {
            return false;
        };
        insert_images_at_text_col(&mut editor_row.item, insert_col, media);

        let new_top = reconstruct_top_level(&self.rows);
        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        self.selection = TextSelection::collapsed(TextLocation {
            row,
            col: insert_col.min(self.line_len(row)),
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        self.emit_top_level_diff(&old_top, &new_top, cx);
        cx.notify();
        true
    }

    /// Insert `blocks` (image/table inlines) at `col` on a *document* line,
    /// splitting the line's text around them so each block becomes its own line.
    pub(super) fn insert_block_lines_at_doc_row(
        &mut self,
        row: usize,
        col: usize,
        blocks: Vec<Inline>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || blocks.is_empty() || self.rows.is_empty() {
            return false;
        }
        let row = row.min(self.rows.len() - 1);
        let old_top = reconstruct_top_level(&self.rows);
        let Some(pos) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };
        let mut new_top = old_top.clone();
        let Some(orig) = new_top.get(pos) else {
            return false;
        };
        let replacement = split_line_with_blocks(orig, col, blocks);
        let inserted = replacement.len();
        new_top.splice(pos..=pos, replacement);

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        // Caret just after the inserted block(s): the start of the trailing text
        // line when one exists, otherwise the end of the last block line.
        let cursor_top = pos + inserted.saturating_sub(1);
        let target_row = flat_row_for_top_level_index(&self.rows, cursor_top);
        let col = if self
            .rows
            .get(target_row)
            .is_some_and(|r| item_has_block_object(&r.item))
        {
            self.line_len(target_row)
        } else {
            0
        };
        self.selection = TextSelection::collapsed(TextLocation {
            row: target_row,
            col,
        });
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        self.emit_top_level_diff(&old_top, &new_top, cx);
        cx.notify();
        true
    }

    /// Paste a run captured from a text+block selection, splicing it into the
    /// caret line so a cut immediately followed by a paste restores the original.
    pub(super) fn paste_spliced_items(
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

    pub(super) fn paste_rich_objects(
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

fn insert_images_at_text_col(item: &mut Item, col: usize, images: Vec<ImageInline>) {
    let mut remaining = col;
    let mut inserted = false;
    let existing = std::mem::take(&mut item.content).to_inlines();
    let mut output = Vec::with_capacity(existing.len() + images.len());
    let mut image_inlines = images.into_iter().map(Inline::Image).collect::<Vec<_>>();

    for inline in existing {
        match inline {
            Inline::Text { text } if !inserted => {
                if remaining <= text.len() {
                    let split = previous_char_boundary_at(&text, remaining);
                    if split > 0 {
                        output.push(Inline::text(text[..split].to_string()));
                    }
                    output.append(&mut image_inlines);
                    if split < text.len() {
                        output.push(Inline::text(text[split..].to_string()));
                    }
                    inserted = true;
                } else {
                    remaining = remaining.saturating_sub(text.len());
                    output.push(Inline::text(text));
                }
            }
            other => output.push(other),
        }
    }

    if !inserted {
        output.append(&mut image_inlines);
    }

    item.content = ItemContent::from_inlines(output);
}

fn previous_char_boundary_at(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}
