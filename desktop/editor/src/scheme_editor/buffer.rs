use knotq_model::{ColumnId, Item, ItemContent, ItemId, Table};
use uuid::Uuid;

mod block_content;
mod line_text;
mod row;
mod builders;

pub(in crate::scheme_editor) use block_content::*;
pub(in crate::scheme_editor) use line_text::*;
pub(in crate::scheme_editor) use row::*;
pub(in crate::scheme_editor) use builders::*;

pub(in crate::scheme_editor) const TABLE_OBJECT_CHAR: char = '\u{fffc}';
pub(in crate::scheme_editor) const TABLE_OBJECT_LEN: usize = TABLE_OBJECT_CHAR.len_utf8();

/// Sentinel table-row index for a header cell. Header cells map to a column's
/// *name* rather than a body row, so they live "above" body row 0 — and
/// `HEADER_ROW as isize == -1` makes vertical navigation fall out naturally.
pub(in crate::scheme_editor) const HEADER_ROW: usize = usize::MAX;

pub(in crate::scheme_editor) fn item_has_block_object(item: &Item) -> bool {
    item.has_table() || item.has_images()
}

pub(in crate::scheme_editor) fn rows_have_block_object(rows: &[EditorRow]) -> bool {
    rows.iter().any(|row| item_has_block_object(&row.item))
}

pub(in crate::scheme_editor) fn flat_row_for_top_level_index(
    rows: &[EditorRow],
    top_level_index: usize,
) -> usize {
    let mut top = 0;
    for (row, editor_row) in rows.iter().enumerate() {
        if editor_row.path.is_cell() {
            continue;
        }
        if top == top_level_index {
            return row;
        }
        top += 1;
    }
    rows.len().saturating_sub(1)
}

pub(in crate::scheme_editor) fn top_level_index_for_flat_row(
    rows: &[EditorRow],
    target_row: usize,
) -> Option<usize> {
    let mut top = 0;
    for (row, editor_row) in rows.iter().enumerate() {
        if editor_row.path.is_cell() {
            continue;
        }
        if row == target_row {
            return Some(top);
        }
        top += 1;
    }
    None
}

pub(in crate::scheme_editor) fn same_rows(a: &[EditorRow], b: &[EditorRow]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.item.id == b.item.id
                && a.path == b.path
                && a.item.content == b.item.content
                && a.item.marker == b.item.marker
                && a.item.indent == b.item.indent
                && a.item.start == b.item.start
                && a.item.end == b.item.end
                && a.item.available == b.item.available
                && a.item.repeats == b.item.repeats
                && a.item.priority == b.item.priority
                && same_item_state(&a.item, &b.item)
        })
}

fn same_item_state(a: &Item, b: &Item) -> bool {
    a.state.len() == b.state.len()
        && a.state
            .iter()
            .zip(&b.state)
            .all(|(a, b)| a.occurrence == b.occurrence && a.state.progress == b.state.progress)
}

#[cfg(test)]
mod tests;
