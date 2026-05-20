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

        let old_text_lines: Vec<String> = self
            .rows
            .iter()
            .map(|row| clean_line_text(&row.item.text))
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

        if reuse_first {
            if let Some(item) = items.get_mut(prefix) {
                let text = new_text_lines[prefix].clone();
                if item.text != text {
                    item.text = text.clone();
                    commands.push(Command::UpdateItemText {
                        scheme: self.scheme_id,
                        item: item.id,
                        text,
                    });
                }
            }
        }

        let delete_start = if reuse_first { prefix + 1 } else { prefix };
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

    fn try_auto_bulletize(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
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
        let text = &editor_row.item.text;

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
        let original_text = editor_row.item.text.clone();
        let original_marker = editor_row.item.marker;
        self.auto_bullet_undo = Some((row, original_text, original_marker));

        // Strip the prefix and set the marker.
        let new_text = editor_row.item.text[prefix_len..].to_string();
        let item = &mut self.rows[row].item;
        item.text = new_text.clone();
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
