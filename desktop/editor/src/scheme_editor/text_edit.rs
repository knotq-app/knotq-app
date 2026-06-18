use super::*;

impl SchemeEditor {
    pub(super) fn replace_selection(
        &mut self,
        replacement: &str,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let (start, end) = self.selection_offsets();
        self.replace_byte_range(start..end, replacement, window, cx);
    }

    pub(super) fn replace_byte_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.replace_byte_range_with_style_inference(range, replacement, true, window, cx);
    }

    pub(super) fn replace_byte_range_with_style_inference(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        infer_inserted_line_style: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        // Clear auto-bulletize undo on any new text edit (auto-bulletize will
        // re-set it if the edit triggers a conversion).
        self.auto_bullet_undo = None;
        let mut text = self.text.clone();
        let start = range.start.min(text.len());
        let end = range.end.min(text.len());
        if start > end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return;
        }
        let inserted_line_hint = if infer_inserted_line_style && replacement.contains('\n') {
            let start_loc = self.offset_to_location(start);
            let insert_before_current = start_loc.col == 0 && self.line_len(start_loc.row) > 0;
            let inserted_row = if insert_before_current {
                start_loc.row
            } else {
                start_loc.row + 1
            };
            self.rows.get(start_loc.row).map(|row| InsertedLineHint {
                style: InsertedLineStyle::from_item(&row.item),
                insert_at: inserted_row,
                first_new_line: inserted_row,
            })
        } else {
            None
        };

        let replacement = replacement.replace('\t', " ");
        text.replace_range(start..end, &replacement);
        let new_offset = start + replacement.len();
        let cursor_after = self.offset_to_location_in(&text, new_offset);
        self.sync_text_from_buffer(
            text,
            cursor_after,
            inserted_line_hint,
            infer_inserted_line_style,
            window,
            cx,
        );
    }

    pub(super) fn sync_text_from_buffer(
        &mut self,
        new_text: String,
        cursor_after: TextLocation,
        inserted_line_hint: Option<InsertedLineHint>,
        infer_inserted_line_style: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        if new_text == self.text {
            self.selection = TextSelection::collapsed(self.clamp_location(cursor_after));
            self.reset_cursor_blink(cx);
            self.scroll_to_cursor(cx);
            cx.notify();
            return;
        }

        if rows_have_block_object(&self.rows) {
            self.sync_tabled_buffer(new_text, cursor_after, window, cx);
            return;
        }

        let old_text_lines: Vec<String> = self
            .rows
            .iter()
            .map(|row| clean_line_text(&row.item.text()))
            .collect();
        let new_text_lines: Vec<String> = new_text.split('\n').map(clean_line_text).collect();
        let old_refs: Vec<&str> = old_text_lines.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new_text_lines.iter().map(String::as_str).collect();
        let change = line_change(&old_refs, &new_refs);

        let prefix = change.prefix;
        let old_suffix = change.old_suffix;
        let new_suffix = change.new_suffix;
        let old_changed = old_suffix.saturating_sub(prefix);
        let new_changed = new_suffix.saturating_sub(prefix);
        let reuse_first = old_changed > 0 && new_changed > 0;

        let mut items: Vec<Item> = self.rows.iter().map(|row| row.item.clone()).collect();
        let mut commands = Vec::new();

        let delete_start = if reuse_first { prefix + 1 } else { prefix };
        // Lines that merge into the reused prefix line carry their embeds
        // (inline images/tables) onto it rather than being dropped along with
        // their deleted items.
        let merged_embeds: Vec<Inline> = if reuse_first {
            (delete_start..old_suffix.min(items.len()))
                .flat_map(|idx| {
                    items[idx]
                        .content
                        .iter()
                        .filter(|inline| !inline.is_text())
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .collect()
        } else {
            Vec::new()
        };

        if reuse_first {
            if let Some(item) = items.get_mut(prefix) {
                let text = new_text_lines[prefix].clone();
                let text_changed = item.text() != text;
                if text_changed {
                    item.set_text(text.clone());
                }
                if merged_embeds.is_empty() {
                    if text_changed {
                        commands.push(Command::UpdateItemText {
                            scheme: self.scheme_id,
                            item: item.id,
                            text,
                        });
                    }
                } else {
                    item.content.extend(merged_embeds.iter().cloned());
                    commands.push(Command::ReplaceItem {
                        scheme: self.scheme_id,
                        item: item.clone(),
                    });
                }
            }
        }

        for _ in delete_start..old_suffix {
            if delete_start < items.len() {
                let id = items[delete_start].id;
                items.remove(delete_start);
                commands.push(Command::DeleteItem {
                    scheme: self.scheme_id,
                    item: id,
                });
            }
        }

        if let Some(hint) = inserted_line_hint.filter(|_| old_changed == 0 && new_changed > 0) {
            let insert_start = hint.insert_at.min(items.len());
            let first_new = hint.first_new_line.min(new_text_lines.len());
            let line_count = new_changed.min(new_text_lines.len().saturating_sub(first_new));
            for i in 0..line_count {
                let insert_at = insert_start + i;
                let new_item =
                    item_for_inserted_line(new_text_lines[first_new + i].clone(), Some(hint.style));
                items.insert(insert_at, new_item.clone());
                commands.push(Command::InsertItem {
                    scheme: self.scheme_id,
                    position: insert_at,
                    item: new_item,
                });
            }
        } else {
            let insert_start = if reuse_first { prefix + 1 } else { prefix };
            let first_new = if reuse_first { prefix + 1 } else { prefix };
            for (line_idx, line) in new_text_lines
                .iter()
                .enumerate()
                .take(new_suffix)
                .skip(first_new)
            {
                let insert_at = insert_start + (line_idx - first_new);
                let style = if infer_inserted_line_style {
                    inserted_line_hint
                        .map(|hint| hint.style)
                        .or_else(|| inserted_line_style_for_position(&items, insert_at))
                } else {
                    None
                };
                let new_item = item_for_inserted_line(line.clone(), style);
                items.insert(insert_at, new_item.clone());
                commands.push(Command::InsertItem {
                    scheme: self.scheme_id,
                    position: insert_at,
                    item: new_item,
                });
            }
        }

        let (text, rows) = build_buffer(&items);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        self.selection = TextSelection::collapsed(self.clamp_location(cursor_after));
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();

        if let Some(cmd) = Command::from_vec(commands) {
            cx.emit(EditorEvent::Command(cmd));
        }

        // Auto-bulletize: detect "- ", "* ", or "N. " at line start on Blank lines.
        self.try_auto_bulletize(cx);

        cx.notify();
    }

    fn sync_tabled_buffer(
        &mut self,
        new_text: String,
        cursor_after: TextLocation,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let old_rows = self.rows.clone();
        let old_lines: Vec<String> = old_rows.iter().map(display_line_for_row).collect();
        let new_lines: Vec<String> = new_text.split('\n').map(clean_display_line_text).collect();
        let old_refs: Vec<&str> = old_lines.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new_lines.iter().map(String::as_str).collect();
        let change = line_change(&old_refs, &new_refs);

        let new_rows = rebuild_tabled_rows_after_text_change(
            &old_rows,
            &new_lines,
            change,
            self.selection.head,
        );

        let old_top = reconstruct_top_level(&old_rows);
        let new_top = reconstruct_top_level(&new_rows);

        let (text, rows) = build_buffer(&new_top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(window);
        self.selection = TextSelection::collapsed(self.clamp_location(cursor_after));
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        self.scroll_to_cursor(cx);
        cx.notify();

        self.emit_top_level_diff(&old_top, &new_top, cx);
    }

    pub(super) fn emit_top_level_diff(
        &mut self,
        old: &[Item],
        new: &[Item],
        cx: &mut Context<Self>,
    ) {
        use std::collections::{HashMap, HashSet};

        let new_ids: HashSet<ItemId> = new.iter().map(|item| item.id).collect();
        let old_by_id: HashMap<ItemId, &Item> = old.iter().map(|item| (item.id, item)).collect();
        let mut commands = Vec::new();

        for item in old {
            if !new_ids.contains(&item.id) {
                commands.push(Command::DeleteItem {
                    scheme: self.scheme_id,
                    item: item.id,
                });
            }
        }

        for (position, item) in new.iter().enumerate() {
            match old_by_id.get(&item.id) {
                Some(previous) => {
                    if **previous != *item {
                        commands.push(Command::ReplaceItem {
                            scheme: self.scheme_id,
                            item: item.clone(),
                        });
                    }
                }
                None => {
                    commands.push(Command::InsertItem {
                        scheme: self.scheme_id,
                        position,
                        item: item.clone(),
                    });
                }
            }
        }

        if let Some(command) = Command::from_vec(commands) {
            cx.emit(EditorEvent::Command(command));
        }
    }

    fn try_auto_bulletize(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if rows_have_block_object(&self.rows) {
            return;
        }
        // Only act when cursor is collapsed (no selection).
        if !self.selection.is_empty() {
            return;
        }
        let row = self.selection.head.row;
        let Some(editor_row) = self.rows.get(row) else {
            return;
        };
        // Only auto-convert Blank lines.
        if editor_row.item.marker != ItemMarker::Blank {
            return;
        }
        let text = editor_row.item.text();
        let text = text.as_str();

        let (new_marker, prefix_len) = if text == "- " || text == "* " {
            (ItemMarker::Bullet, text.len())
        } else if let Some(rest) = text.strip_prefix(|c: char| c.is_ascii_digit()) {
            // Match patterns like "1. ", "2. ", etc.
            if let Some(after_dots) = rest.strip_prefix(". ") {
                // Only convert if the entire text is "N. " (digits + ". ")
                let digit_count = text.len() - rest.len();
                let full_prefix = digit_count + 2; // digits + ". "
                if after_dots.is_empty() {
                    (ItemMarker::Numbered, full_prefix)
                } else {
                    return;
                }
            } else {
                return;
            }
        } else {
            return;
        };

        // Save undo state before conversion.
        let original_text = editor_row.item.text();
        let original_marker = editor_row.item.marker;
        self.auto_bullet_undo = Some((row, original_text.clone(), original_marker));

        // Strip the prefix and set the marker.
        let new_text = original_text[prefix_len..].to_string();
        let item = &mut self.rows[row].item;
        item.set_text(new_text.clone());
        item.marker = new_marker;

        // Rebuild buffer to sync text representation.
        let items: Vec<Item> = self.rows.iter().map(|r| r.item.clone()).collect();
        let (text, rows) = build_buffer(&items);
        self.text = text;
        self.rows = rows;
        self.selection = TextSelection::collapsed(TextLocation { row, col: 0 });
        self.marked_range = None;

        // Emit command to persist the change.
        let item = self.rows[row].item.clone();
        cx.emit(EditorEvent::Command(Command::ReplaceItem {
            scheme: self.scheme_id,
            item,
        }));
    }
}

fn rebuild_tabled_rows_after_text_change(
    old_rows: &[EditorRow],
    new_lines: &[String],
    change: LineChange,
    selection_head: TextLocation,
) -> Vec<EditorRow> {
    let old_changed = change.old_suffix.saturating_sub(change.prefix);
    let new_changed = change.new_suffix.saturating_sub(change.prefix);
    let inserted_path = table_inserted_row_path(old_rows, change, selection_head);
    let inserted_style = table_inserted_row_style(old_rows, change, selection_head);

    let mut new_rows = Vec::with_capacity(new_lines.len());
    for (i, line) in new_lines.iter().enumerate().take(change.prefix) {
        let Some(old_row) = old_rows.get(i) else {
            continue;
        };
        let mut row = old_row.clone();
        set_row_text_from_buffer_line(&mut row, line);
        new_rows.push(row);
    }

    for offset in 0..new_changed {
        let line_index = change.prefix + offset;
        let Some(line) = new_lines.get(line_index) else {
            continue;
        };
        if offset < old_changed {
            if let Some(old_row) = old_rows.get(change.prefix + offset) {
                let mut row = old_row.clone();
                set_row_text_from_buffer_line(&mut row, line);
                new_rows.push(row);
                continue;
            }
        }

        new_rows.push(EditorRow {
            item: item_for_inserted_line(line.clone(), inserted_style),
            path: inserted_path,
        });
    }

    for i in change.old_suffix..old_rows.len() {
        let mut row = old_rows[i].clone();
        let new_index = change.new_suffix + (i - change.old_suffix);
        if let Some(line) = new_lines.get(new_index) {
            set_row_text_from_buffer_line(&mut row, line);
        }
        new_rows.push(row);
    }

    new_rows
}

fn set_row_text_from_buffer_line(row: &mut EditorRow, line: &str) {
    if row.path.is_table_anchor() {
        let table = row
            .item
            .table()
            .cloned()
            .unwrap_or_else(|| knotq_model::Table::new(1, 1));
        set_table_anchor_content_from_line(&mut row.item, line, table);
    } else if row.item.has_images() {
        set_item_content_from_block_line(&mut row.item, line, None);
    } else {
        row.item.set_text(clean_line_text(line));
    }
}

fn table_inserted_row_path(
    old_rows: &[EditorRow],
    change: LineChange,
    selection_head: TextLocation,
) -> RowPath {
    if let Some(row) = old_rows
        .get(selection_head.row)
        .filter(|row| row.path.is_cell())
    {
        return row.path;
    }

    [
        old_rows.get(change.prefix),
        change
            .prefix
            .checked_sub(1)
            .and_then(|index| old_rows.get(index)),
        old_rows.get(change.old_suffix),
    ]
    .into_iter()
    .flatten()
    .find_map(|row| row.path.is_cell().then_some(row.path))
    .unwrap_or_default()
}

fn table_inserted_row_style(
    old_rows: &[EditorRow],
    change: LineChange,
    selection_head: TextLocation,
) -> Option<InsertedLineStyle> {
    [
        old_rows.get(selection_head.row),
        old_rows.get(change.prefix),
        change
            .prefix
            .checked_sub(1)
            .and_then(|index| old_rows.get(index)),
        old_rows.get(change.old_suffix),
    ]
    .into_iter()
    .flatten()
    .find(|row| !row.path.is_table_anchor())
    .map(|row| InsertedLineStyle::from_item(&row.item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::Table;

    fn table_item(rows: usize, cols: usize) -> Item {
        let mut item = Item::new("");
        item.content.push(Inline::Table(Table::new(rows, cols)));
        item
    }

    fn text_lines(text: &str) -> Vec<String> {
        text.split('\n').map(clean_line_text).collect()
    }

    /// Index of the first body cell row (after the anchor and the header row).
    fn first_body_cell(rows: &[EditorRow]) -> usize {
        rows.iter()
            .position(|row| row.path.is_cell() && !row.path.is_header_cell())
            .expect("table has a body cell")
    }

    #[test]
    fn tabled_text_replacement_preserves_the_edited_cell_path() {
        let item = table_item(2, 2);
        let (old_text, old_rows) = build_buffer(&[item]);
        let body0 = first_body_cell(&old_rows);
        let mut new_lines = text_lines(&old_text);
        new_lines[body0] = "Alpha".to_string();
        let old_lines = text_lines(&old_text);
        let old_refs: Vec<&str> = old_lines.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new_lines.iter().map(String::as_str).collect();
        let change = line_change(&old_refs, &new_refs);

        let rows = rebuild_tabled_rows_after_text_change(
            &old_rows,
            &new_lines,
            change,
            TextLocation { row: body0, col: 0 },
        );
        assert!(rows[body0].path.is_cell());
        assert_eq!((rows[body0].path.r, rows[body0].path.c), (0, 0));

        let top = reconstruct_top_level(&rows);
        assert_eq!(top.len(), 1);
        let table = top[0].table().unwrap();
        assert_eq!(table.cell(0, 0).unwrap().items[0].text(), "Alpha");
        assert_eq!(table.cell(0, 1).unwrap().items[0].text(), "");
    }

    #[test]
    fn tabled_line_insertion_uses_the_active_cell_path() {
        let mut item = table_item(2, 2);
        item.table_mut().unwrap().cell_mut(0, 0).unwrap().items[0].set_text("Alpha".to_string());
        let (old_text, old_rows) = build_buffer(&[item]);
        let body0 = first_body_cell(&old_rows);
        let mut new_lines = text_lines(&old_text);
        new_lines.insert(body0 + 1, "Second line".to_string());
        let old_lines = text_lines(&old_text);
        let old_refs: Vec<&str> = old_lines.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new_lines.iter().map(String::as_str).collect();
        let change = line_change(&old_refs, &new_refs);

        let rows = rebuild_tabled_rows_after_text_change(
            &old_rows,
            &new_lines,
            change,
            TextLocation { row: body0, col: 5 },
        );
        let top = reconstruct_top_level(&rows);
        let table = top[0].table().unwrap();

        assert_eq!(table.cell(0, 0).unwrap().items.len(), 2);
        assert_eq!(table.cell(0, 0).unwrap().items[0].text(), "Alpha");
        assert_eq!(table.cell(0, 0).unwrap().items[1].text(), "Second line");
        assert_eq!(table.cell(0, 1).unwrap().items[0].text(), "");
    }
}
