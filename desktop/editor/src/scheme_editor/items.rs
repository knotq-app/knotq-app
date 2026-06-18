use std::ops::Range;

use knotq_model::{Item, ItemMarker};

use crate::line_map::TextLocation;

use super::buffer::{clean_line_text, line_ranges, EditorRow};
use super::selection::TextSelection;
use super::MAX_INDENT;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InsertedLineStyle {
    pub(super) marker: ItemMarker,
    pub(super) indent: u8,
}

impl InsertedLineStyle {
    pub(super) fn from_item(item: &Item) -> Self {
        Self {
            marker: item.marker,
            indent: item.indent,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InsertedLineHint {
    pub(super) style: InsertedLineStyle,
    pub(super) insert_at: usize,
    pub(super) first_new_line: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EmptyLineDeletePlan {
    pub(super) delete_row: usize,
    pub(super) cursor_after: TextLocation,
}

pub(super) fn text_end_location(text: &str) -> TextLocation {
    let ranges = line_ranges(text);
    let Some((row, range)) = ranges.iter().enumerate().next_back() else {
        return TextLocation { row: 0, col: 0 };
    };
    TextLocation {
        row,
        col: range.end.saturating_sub(range.start),
    }
}

pub(super) fn empty_line_delete_plan(
    row: usize,
    row_count: usize,
    prefer_backward: bool,
    previous_line_len: usize,
) -> Option<EmptyLineDeletePlan> {
    if row_count <= 1 || row >= row_count {
        return None;
    }

    let cursor_after = if prefer_backward && row > 0 {
        TextLocation {
            row: row - 1,
            col: previous_line_len,
        }
    } else if !prefer_backward && row + 1 < row_count {
        TextLocation { row, col: 0 }
    } else if row > 0 {
        TextLocation {
            row: row - 1,
            col: previous_line_len,
        }
    } else {
        TextLocation { row: 0, col: 0 }
    };

    Some(EmptyLineDeletePlan {
        delete_row: row,
        cursor_after,
    })
}

pub(super) fn item_for_rich_paste(mut item: Item) -> Item {
    item.id = knotq_model::ItemId::new();
    item.set_text(clean_line_text(&item.text()));
    item.indent = item.indent.min(MAX_INDENT);
    item.enforce_marker_constraints();
    item
}

pub(super) fn whole_row_selection_range(
    selection: TextSelection,
    line_lens: &[usize],
) -> Option<Range<usize>> {
    if selection.is_empty() || line_lens.is_empty() {
        return None;
    }

    let (start, end) = selection.ordered();
    if start.row >= line_lens.len() || end.row >= line_lens.len() || start.col != 0 {
        return None;
    }

    let end_col = end.col.min(line_lens[end.row]);
    if end_col == line_lens[end.row] {
        let end_exclusive = end.row + 1;
        if start.row < end_exclusive {
            return Some(start.row..end_exclusive);
        }
    }

    if end_col == 0 && end.row > start.row {
        let end_exclusive = end.row;
        if start.row < end_exclusive {
            return Some(start.row..end_exclusive);
        }
    }

    None
}

pub(super) fn item_has_line_attributes(item: &Item) -> bool {
    item.indent != 0
        || item.marker != ItemMarker::Blank
        || item.start.is_some()
        || item.end.is_some()
        || item.available.is_some()
        || item.repeats.is_some()
        || item.priority.is_some()
        || item.state.len() != 1
        || item.state.iter().any(|state| !state.state.is_default())
}

pub(super) fn inserted_line_style_for_position(
    items: &[Item],
    insert_at: usize,
) -> Option<InsertedLineStyle> {
    if insert_at > 0 {
        items
            .get(insert_at - 1)
            .map(InsertedLineStyle::from_item)
            .or_else(|| items.get(insert_at).map(InsertedLineStyle::from_item))
    } else {
        items.get(insert_at).map(InsertedLineStyle::from_item)
    }
}

pub(super) fn item_for_inserted_line(text: String, style: Option<InsertedLineStyle>) -> Item {
    let mut item = Item::new(text);
    if let Some(style) = style {
        item.marker = style.marker;
        item.indent = style.indent;
    }
    item
}

pub(super) fn item_with_marker(mut item: Item, marker: ItemMarker) -> Item {
    item.marker = marker;
    item.enforce_marker_constraints();
    item
}

pub(super) fn numbered_marker_ordinal(rows: &[EditorRow], row: usize) -> Option<usize> {
    let current = rows.get(row)?;
    if current.item.marker != ItemMarker::Numbered {
        return None;
    }

    let indent = current.item.indent;
    let mut ordinal = 1;
    for previous in rows[..row].iter().rev() {
        if previous.item.indent > indent {
            continue;
        }
        if previous.item.indent < indent {
            break;
        }
        if previous.item.marker != ItemMarker::Numbered {
            break;
        }
        ordinal += 1;
    }

    Some(ordinal)
}

pub(super) fn item_without_line_attributes(item: &Item) -> Item {
    let mut clean = Item::new("");
    clean.id = item.id;
    clean.content = item.content.clone();
    clean
}
