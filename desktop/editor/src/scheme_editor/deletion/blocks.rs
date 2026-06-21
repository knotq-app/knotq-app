use super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn handle_block_object_enter(
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
        if editor_row.path.is_cell() || !item_has_block_object(&editor_row.item) {
            return false;
        }
        let Some(top_index) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };

        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let mut col = self.selection.head.col.min(self.line_len(row));
        if let Some(object) = self.last_block_object_range_for_row(row) {
            if col > object.start {
                return self.insert_blank_after_block_item(row, window, cx);
            }
            col = col.min(object.start);
        }
        let Some(result) = split_table_item_at_text_col(&mut new_top, top_index, col) else {
            return false;
        };

        self.apply_top_level_edit(&old_top, new_top, window, cx, |this| {
            let row = this
                .rows
                .iter()
                .position(|row| item_has_block_object(&row.item) && row.item.id == result.table)
                .unwrap_or_else(|| flat_row_for_top_level_index(&this.rows, result.table_index));
            TextLocation { row, col: 0 }
        });
        true
    }

    fn insert_blank_after_block_item(
        &mut self,
        block_row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(anchor) = self.rows.get(block_row) else {
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

        self.apply_top_level_edit(&old_top, new_top, window, cx, |this| TextLocation {
            row: flat_row_for_top_level_index(&this.rows, pos + 1),
            col: 0,
        });
        true
    }

    pub(in crate::scheme_editor) fn delete_selected_block_object(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        if let Some(top_indices) =
            selected_cross_row_block_top_indices(&self.rows, &self.text, self.selection)
        {
            return self.delete_top_level_block_items(
                top_indices,
                self.selection.ordered().0,
                window,
                cx,
            );
        }
        let (start, end) = self.selection.ordered();
        if start.row != end.row {
            return false;
        }
        let Some((object, block_index)) = self.block_object_after_or_at(start.row, start.col)
        else {
            return false;
        };
        if start.col != object.start || end.col != object.end {
            return false;
        }
        self.delete_block_object_from_row(start.row, block_index, object.start, window, cx)
    }

    fn delete_block_object_from_row(
        &mut self,
        row: usize,
        block_index: usize,
        cursor_col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let Some(pos) = top_level_index_for_flat_row(&self.rows, row) else {
            return false;
        };
        let Some(item) = new_top.get_mut(pos) else {
            return false;
        };
        let mut seen = 0;
        let mut inlines = item.content.to_inlines();
        let Some(inline_pos) = inlines.iter().position(|inline| {
            if inline.is_text() {
                return false;
            }
            let matches = seen == block_index;
            seen += 1;
            matches
        }) else {
            return false;
        };
        inlines.remove(inline_pos);
        item.content = ItemContent::from_inlines(inlines);

        self.apply_top_level_edit(&old_top, new_top, window, cx, |this| {
            let row = flat_row_for_top_level_index(&this.rows, pos);
            TextLocation {
                row,
                col: cursor_col.min(this.line_len(row)),
            }
        });
        true
    }

    pub(in crate::scheme_editor) fn delete_adjacent_block_item_at_boundary(
        &mut self,
        prefer_backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || !self.selection.is_empty() {
            return false;
        }
        let cursor = self.selection.head;
        let Some(top_index) =
            adjacent_block_top_index_at_boundary(&self.rows, &self.text, cursor, prefer_backward)
        else {
            return false;
        };
        self.delete_top_level_block_items(vec![top_index], cursor, window, cx)
    }

    fn delete_top_level_block_items(
        &mut self,
        mut top_indices: Vec<usize>,
        cursor: TextLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let old_top = reconstruct_top_level(&self.rows);
        top_indices.sort_unstable();
        top_indices.dedup();
        top_indices.retain(|index| {
            old_top
                .get(*index)
                .is_some_and(|item| item_has_block_object(item))
        });
        if top_indices.is_empty() {
            return false;
        }

        let old_cursor_top = top_level_index_for_cursor_row(&self.rows, cursor.row);
        let mut new_top = old_top.clone();
        for index in top_indices.iter().rev() {
            if *index < new_top.len() {
                new_top.remove(*index);
            }
        }
        let new_top_len = new_top.len();

        self.apply_top_level_edit(&old_top, new_top, window, cx, |this| {
            this.cursor_after_top_level_deletion(cursor, old_cursor_top, &top_indices, new_top_len)
        });
        true
    }

    fn cursor_after_top_level_deletion(
        &self,
        cursor: TextLocation,
        old_cursor_top: Option<usize>,
        deleted_top_indices: &[usize],
        new_top_len: usize,
    ) -> TextLocation {
        let Some(old_cursor_top) = old_cursor_top else {
            return self.clamp_location(cursor);
        };
        if new_top_len == 0 {
            return TextLocation { row: 0, col: 0 };
        }

        let deleted_before = deleted_top_indices
            .iter()
            .filter(|index| **index < old_cursor_top)
            .count();
        let target_top = old_cursor_top
            .saturating_sub(deleted_before)
            .min(new_top_len.saturating_sub(1));
        let row = flat_row_for_top_level_index(&self.rows, target_top);
        let col = if deleted_top_indices.binary_search(&old_cursor_top).is_ok() {
            0
        } else {
            cursor.col
        };
        self.clamp_location(TextLocation { row, col })
    }

    fn block_object_after_or_at(&self, row: usize, col: usize) -> Option<(Range<usize>, usize)> {
        let line = self
            .line_range(row)
            .and_then(|range| self.text.get(range))?;
        block_object_ranges(line)
            .into_iter()
            .enumerate()
            .find(|(_, object)| col <= object.start)
            .map(|(index, object)| (object, index))
    }

    /// A collapsed-caret delete sitting at the edge of a whole-line block
    /// (image/table) removes the block, leaving an empty text line in its place
    /// (the line keeps its identity/marker/dates). Backspace deletes the block
    /// just before the caret; forward-delete the block just after it.
    pub(in crate::scheme_editor) fn delete_block_object_at_caret(
        &mut self,
        prefer_backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || !self.selection.is_empty() {
            return false;
        }
        let head = self.selection.head;
        let Some(current) = self.rows.get(head.row) else {
            return false;
        };
        // Only document-level blocks are atomic this way; inside a table cell,
        // editing is ordinary text.
        if current.path.is_cell() || !item_has_block_object(&current.item) {
            return false;
        }
        let col = head.col.min(self.line_len(head.row));
        let Some(line) = self
            .line_range(head.row)
            .and_then(|range| self.text.get(range))
        else {
            return false;
        };
        let Some((object, block_index)) =
            block_object_adjacent_to_caret(line, col, prefer_backward)
        else {
            return false;
        };
        self.delete_block_object_from_row(head.row, block_index, object.start, window, cx)
    }

    pub(in crate::scheme_editor) fn merge_adjacent_block_if_boundary(
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
            if col != 0 {
                None
            } else if self
                .rows
                .get(row)
                .is_some_and(|row| item_has_block_object(&row.item))
            {
                top_level_index_for_flat_row(&self.rows, row).and_then(|table_top| {
                    table_top.checked_sub(1).map(|target_top| {
                        let target_row = flat_row_for_top_level_index(&self.rows, target_top);
                        (target_top, table_top, self.line_len(target_row))
                    })
                })
            } else if path.is_doc() {
                top_level_index_for_flat_row(&self.rows, row).and_then(|item_top| {
                    let table_top = item_top.checked_sub(1)?;
                    let table_row = flat_row_for_top_level_index(&self.rows, table_top);
                    self.rows
                        .get(table_row)
                        .filter(|row| item_has_block_object(&row.item))
                        .map(|_| (item_top, table_top, self.line_len(table_row)))
                })
            } else {
                None
            }
        } else if col != self.line_len(row) {
            None
        } else {
            top_level_index_for_flat_row(&self.rows, row).and_then(|target_top| {
                let table_top = target_top.checked_add(1)?;
                let table_row = flat_row_for_top_level_index(&self.rows, table_top);
                self.rows
                    .get(table_row)
                    .filter(|row| item_has_block_object(&row.item))
                    .map(|_| (target_top, table_top, col))
            })
        }) else {
            return false;
        };

        let old_top = reconstruct_top_level(&self.rows);
        let mut new_top = old_top.clone();
        let result = if prefer_backward && target_top > table_top {
            append_item_into_table(&mut new_top, table_top, target_top)
        } else {
            merge_table_item_into(&mut new_top, target_top, table_top)
        };
        let Some(result) = result else {
            return false;
        };

        self.apply_top_level_edit(&old_top, new_top, window, cx, |this| {
            let row = flat_row_for_top_level_index(&this.rows, result.target_index);
            TextLocation {
                row,
                col: cursor_col.min(this.line_len(row)),
            }
        });
        true
    }
}
