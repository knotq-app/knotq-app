use super::*;

pub(in crate::scheme_editor::deletion) fn is_empty_doc_line_adjacent_to_block(
    rows: &[EditorRow],
    row: usize,
    line_len: usize,
) -> bool {
    let Some(editor_row) = rows.get(row) else {
        return false;
    };
    if !editor_row.path.is_doc() || line_len != 0 {
        return false;
    }

    let Some(top_index) = top_level_index_for_flat_row(rows, row) else {
        return false;
    };
    let top = reconstruct_top_level(rows);
    top_index
        .checked_sub(1)
        .and_then(|previous| top.get(previous))
        .is_some_and(item_has_block_object)
        || top.get(top_index + 1).is_some_and(item_has_block_object)
}

/// The block object a collapsed-caret delete at `col` would remove: the one
/// ending at `col` for a backward delete (caret just after it) or starting at
/// `col` for a forward delete (caret just before it). Returns the object's byte
/// range and its index among the line's block objects.
pub(in crate::scheme_editor::deletion) fn block_object_adjacent_to_caret(
    line: &str,
    col: usize,
    backward: bool,
) -> Option<(Range<usize>, usize)> {
    block_object_ranges(line)
        .into_iter()
        .enumerate()
        .find(|(_, object)| {
            if backward {
                object.end == col
            } else {
                object.start == col
            }
        })
        .map(|(index, object)| (object, index))
}

pub(in crate::scheme_editor::deletion) fn same_region(a: RowPath, b: RowPath) -> bool {
    if a.is_cell() && b.is_cell() {
        a.anchor == b.anchor && a.r == b.r && a.c == b.c
    } else {
        a.is_doc() && b.is_doc()
    }
}

pub(in crate::scheme_editor::deletion) fn adjacent_block_top_index_at_boundary(
    rows: &[EditorRow],
    text: &str,
    cursor: TextLocation,
    backward: bool,
) -> Option<usize> {
    let current = rows.get(cursor.row)?;
    if !current.path.is_doc() || item_has_block_object(&current.item) {
        return None;
    }
    let line_len = line_len_in_text(text, cursor.row)?;
    let col = cursor.col.min(line_len);
    let current_top = top_level_index_for_flat_row(rows, cursor.row)?;

    let target_top = if backward {
        (col == 0).then(|| current_top.checked_sub(1)).flatten()?
    } else {
        (col == line_len).then_some(current_top + 1)?
    };
    let target_row = flat_row_for_top_level_index(rows, target_top);
    rows.get(target_row)
        .filter(|row| !row.path.is_cell() && item_has_block_object(&row.item))
        .and_then(|_| top_level_index_for_flat_row(rows, target_row))
}

pub(in crate::scheme_editor::deletion) fn selected_cross_row_block_top_indices(
    rows: &[EditorRow],
    text: &str,
    selection: TextSelection,
) -> Option<Vec<usize>> {
    if selection.is_empty() {
        return None;
    }
    let (start, end) = selection.ordered();
    if start.row == end.row {
        return None;
    }

    let ranges = line_ranges(text);
    let last_row = end.row.min(ranges.len().saturating_sub(1));
    let mut block_rows = Vec::new();
    for row in start.row..=last_row {
        let Some(editor_row) = rows.get(row) else {
            continue;
        };
        if editor_row.path.is_cell() || !item_has_block_object(&editor_row.item) {
            continue;
        }
        let Some((selection_start, selection_end, line)) =
            selected_line_slice(text, &ranges, selection, row)
        else {
            continue;
        };
        if selection_start >= selection_end {
            continue;
        }
        if block_object_ranges(line)
            .into_iter()
            .any(|object| selection_start < object.end && object.start < selection_end)
        {
            block_rows.push(row);
        }
    }
    if block_rows.is_empty() {
        return None;
    }

    for row in start.row..=last_row {
        let Some((selection_start, selection_end, line)) =
            selected_line_slice(text, &ranges, selection, row)
        else {
            continue;
        };
        if selection_start >= selection_end {
            continue;
        }
        if rows
            .get(row)
            .is_some_and(|row| row.path.is_cell() && block_rows.contains(&row.path.anchor))
        {
            continue;
        }
        if !line_without_table_object(&line[selection_start..selection_end])
            .trim()
            .is_empty()
        {
            return None;
        }
    }

    let mut top_indices = block_rows
        .into_iter()
        .filter_map(|row| top_level_index_for_flat_row(rows, row))
        .collect::<Vec<_>>();
    top_indices.sort_unstable();
    top_indices.dedup();
    (!top_indices.is_empty()).then_some(top_indices)
}

fn selected_line_slice<'a>(
    text: &'a str,
    ranges: &[Range<usize>],
    selection: TextSelection,
    row: usize,
) -> Option<(usize, usize, &'a str)> {
    let (start, end) = selection.ordered();
    let range = ranges.get(row)?.clone();
    let line = text.get(range)?;
    let selection_start = if row == start.row { start.col } else { 0 };
    let selection_end = if row == end.row { end.col } else { line.len() };
    let selection_start = clamp_col_to_line_boundary(line, selection_start);
    let selection_end = clamp_col_to_line_boundary(line, selection_end);
    Some((selection_start.min(selection_end), selection_end, line))
}

fn line_len_in_text(text: &str, row: usize) -> Option<usize> {
    line_ranges(text)
        .get(row)
        .map(|range| range.end.saturating_sub(range.start))
}

fn clamp_col_to_line_boundary(line: &str, col: usize) -> usize {
    let mut col = col.min(line.len());
    while col > 0 && !line.is_char_boundary(col) {
        col -= 1;
    }
    col
}

pub(in crate::scheme_editor::deletion) fn top_level_index_for_cursor_row(
    rows: &[EditorRow],
    row: usize,
) -> Option<usize> {
    let editor_row = rows.get(row)?;
    if editor_row.path.is_cell() {
        top_level_index_for_flat_row(rows, editor_row.path.anchor)
    } else {
        top_level_index_for_flat_row(rows, row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::Table;

    #[test]
    fn empty_doc_line_after_table_is_boundary_across_cell_rows() {
        let mut table = Item::new("");
        table.set_table(Table::new(2, 2));
        let blank = Item::new("");
        let (_, rows) = build_buffer(&[table, blank]);
        let blank_row = rows
            .iter()
            .rposition(|row| row.path.is_doc() && !row.item.has_table())
            .expect("blank doc row after table");

        assert!(!rows[blank_row - 1].path.is_table_anchor());
        assert!(is_empty_doc_line_adjacent_to_block(&rows, blank_row, 0));
    }

    #[test]
    fn empty_doc_line_before_table_is_boundary() {
        let blank = Item::new("");
        let mut table = Item::new("");
        table.set_table(Table::new(1, 2));
        let (_, rows) = build_buffer(&[blank, table]);

        assert!(rows[1].path.is_table_anchor());
        assert!(is_empty_doc_line_adjacent_to_block(&rows, 0, 0));
        assert!(!is_empty_doc_line_adjacent_to_block(&rows, 0, 1));
    }

    #[test]
    fn caret_after_block_targets_object_for_backspace() {
        let line = TABLE_OBJECT_CHAR.to_string();
        assert_eq!(
            block_object_adjacent_to_caret(&line, TABLE_OBJECT_LEN, true),
            Some((0..TABLE_OBJECT_LEN, 0))
        );
        // Caret before the object is not a backspace target.
        assert_eq!(block_object_adjacent_to_caret(&line, 0, true), None);
    }

    #[test]
    fn caret_before_block_targets_object_for_forward_delete() {
        let line = TABLE_OBJECT_CHAR.to_string();
        assert_eq!(
            block_object_adjacent_to_caret(&line, 0, false),
            Some((0..TABLE_OBJECT_LEN, 0))
        );
        // Caret after the object is not a forward-delete target.
        assert_eq!(
            block_object_adjacent_to_caret(&line, TABLE_OBJECT_LEN, false),
            None
        );
    }

    #[test]
    fn caret_in_plain_text_has_no_block_object() {
        assert_eq!(block_object_adjacent_to_caret("hello", 5, true), None);
        assert_eq!(block_object_adjacent_to_caret("hello", 0, false), None);
    }
}

#[cfg(test)]
mod block_boundary_tests {
    use super::*;
    use knotq_model::Table;

    fn table_item(rows: usize, cols: usize) -> Item {
        let mut item = Item::new("");
        item.set_table(Table::new(rows, cols));
        item
    }

    #[test]
    fn forward_delete_at_text_table_boundary_targets_the_table_item() {
        let text_item = Item::new("finish my room list and items");
        let (text, rows) = build_buffer(&[text_item, table_item(2, 2)]);

        assert_eq!(
            adjacent_block_top_index_at_boundary(
                &rows,
                &text,
                TextLocation {
                    row: 0,
                    col: "finish my room list and items".len(),
                },
                false,
            ),
            Some(1)
        );
    }

    #[test]
    fn backspace_at_table_text_boundary_targets_the_table_item() {
        let text_item = Item::new("after");
        let (text, rows) = build_buffer(&[table_item(2, 2), text_item]);
        let text_row = rows
            .iter()
            .position(|row| row.path.is_doc() && row.item.text() == "after")
            .unwrap();

        assert_eq!(
            adjacent_block_top_index_at_boundary(
                &rows,
                &text,
                TextLocation {
                    row: text_row,
                    col: 0,
                },
                true,
            ),
            Some(0)
        );
    }

    #[test]
    fn selected_newline_and_table_object_targets_the_table_item() {
        let text_item = Item::new("finish my room list and items");
        let (text, rows) = build_buffer(&[text_item, table_item(2, 2)]);

        assert_eq!(
            selected_cross_row_block_top_indices(
                &rows,
                &text,
                TextSelection {
                    anchor: TextLocation {
                        row: 0,
                        col: "finish my room list and items".len(),
                    },
                    head: TextLocation {
                        row: 1,
                        col: TABLE_OBJECT_LEN,
                    },
                },
            ),
            Some(vec![1])
        );
    }

    #[test]
    fn selected_table_object_and_cell_rows_targets_the_table_item() {
        let text_item = Item::new("finish my room list and items");
        let (text, rows) = build_buffer(&[text_item, table_item(2, 2)]);

        assert_eq!(
            selected_cross_row_block_top_indices(
                &rows,
                &text,
                TextSelection {
                    anchor: TextLocation {
                        row: 0,
                        col: "finish my room list and items".len(),
                    },
                    head: TextLocation {
                        row: 2,
                        col: "Column 2".len(),
                    },
                },
            ),
            Some(vec![1])
        );
    }

    #[test]
    fn selection_with_real_text_before_table_is_not_a_block_delete() {
        let text_item = Item::new("finish my room list and items");
        let (text, rows) = build_buffer(&[text_item, table_item(2, 2)]);

        assert_eq!(
            selected_cross_row_block_top_indices(
                &rows,
                &text,
                TextSelection {
                    anchor: TextLocation {
                        row: 0,
                        col: "finish my room list and item".len(),
                    },
                    head: TextLocation {
                        row: 1,
                        col: TABLE_OBJECT_LEN,
                    },
                },
            ),
            None
        );
    }
}
