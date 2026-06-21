use std::ops::Range;

use knotq_model::{Inline, Item, ItemContent, ItemId, Table};

use super::*;

pub(in crate::scheme_editor) fn set_table_anchor_content_from_line(
    item: &mut Item,
    line: &str,
    table: Table,
) {
    let mut line = clean_display_line_text(line);
    if table_object_range(&line).is_none() {
        line.push(TABLE_OBJECT_CHAR);
    }
    set_item_content_from_block_line(item, &line, Some(table));
}

pub(in crate::scheme_editor) fn set_item_content_from_block_line(
    item: &mut Item,
    line: &str,
    table: Option<Table>,
) {
    let line = clean_display_line_text(line);
    let mut blocks = block_inlines_for_item(item, table).into_iter();
    item.content = ItemContent::from_inlines(content_from_block_line(&line, &mut blocks));
}

fn block_inlines_for_item(item: &Item, table: Option<Table>) -> Vec<Inline> {
    let mut used_table_override = false;
    let mut blocks = item
        .content
        .to_inlines()
        .into_iter()
        .filter_map(|inline| match inline {
            Inline::Image(image) => Some(Inline::Image(image.clone())),
            Inline::Table(existing) => {
                let table = if !used_table_override {
                    used_table_override = true;
                    table.clone().unwrap_or_else(|| existing.clone())
                } else {
                    existing.clone()
                };
                Some(Inline::Table(table))
            }
            Inline::Text { .. } => None,
        })
        .collect::<Vec<_>>();
    if !used_table_override {
        if let Some(table) = table {
            blocks.push(Inline::Table(table));
        }
    }
    blocks
}

pub(in crate::scheme_editor) fn selected_block_inlines(
    item: &Item,
    line: &str,
    selection: Range<usize>,
) -> Vec<Inline> {
    let blocks = block_inlines_for_item(item, None);
    block_object_ranges(line)
        .into_iter()
        .zip(blocks)
        .filter_map(|(object, block)| {
            (selection.start < object.end && object.start < selection.end).then_some(block)
        })
        .collect()
}

pub(in crate::scheme_editor) fn replace_block_range_with_inlines(
    item: &mut Item,
    line: &str,
    range: Range<usize>,
    inserted: Vec<Inline>,
) -> bool {
    if range.start > range.end
        || range.end > line.len()
        || !line.is_char_boundary(range.start)
        || !line.is_char_boundary(range.end)
    {
        return false;
    }

    let old_blocks = block_inlines_for_item(item, None);
    let object_ranges = block_object_ranges(line);
    let mut new_line = String::with_capacity(
        line.len() + inserted.len() * TABLE_OBJECT_LEN - range.len().min(line.len()),
    );
    new_line.push_str(&line[..range.start]);
    for _ in &inserted {
        new_line.push(TABLE_OBJECT_CHAR);
    }
    new_line.push_str(&line[range.end..]);

    let mut blocks = Vec::with_capacity(old_blocks.len() + inserted.len());
    let mut inserted_blocks = Some(inserted);
    for (object, block) in object_ranges.into_iter().zip(old_blocks) {
        if object.end <= range.start {
            blocks.push(block);
        } else if object.start >= range.end {
            if let Some(mut inserted) = inserted_blocks.take() {
                blocks.append(&mut inserted);
            }
            blocks.push(block);
        }
    }
    if let Some(mut inserted) = inserted_blocks {
        blocks.append(&mut inserted);
    }

    item.content =
        ItemContent::from_inlines(content_from_block_line(&new_line, &mut blocks.into_iter()));
    true
}

fn content_from_block_line(line: &str, blocks: &mut impl Iterator<Item = Inline>) -> Vec<Inline> {
    let mut content = Vec::new();
    let mut cursor = 0;
    for object in block_object_ranges(line) {
        let before = line_without_table_object(&line[cursor..object.start]);
        if !before.is_empty() {
            content.push(Inline::text(before));
        }
        if let Some(block) = blocks.next() {
            content.push(block);
        }
        cursor = object.end;
    }
    let after = line_without_table_object(&line[cursor..]);
    if !after.is_empty() {
        content.push(Inline::text(after));
    }
    content
}

/// Split a text line at byte offset `col` and interleave whole-line block items,
/// yielding the resulting single-content items in document order. Leading and
/// trailing text each become their own line *only when non-empty*; every block is
/// its own line. The original item keeps its identity and line metadata on the
/// surviving text part — or, for a text-less line, on the first block — so the
/// line's CRDT identity stays stable. Used when an image/table is inserted into
/// the middle of a text line (paste, drop) so the line splits instead of the
/// block clobbering the text.
pub(in crate::scheme_editor) fn split_line_with_blocks(
    orig: &Item,
    col: usize,
    blocks: Vec<Inline>,
) -> Vec<Item> {
    let block_items: Vec<Item> = blocks
        .into_iter()
        .map(|block| {
            let mut item = Item::new("");
            item.indent = orig.indent;
            item.content = ItemContent::from_inlines(vec![block]);
            item
        })
        .collect();
    if block_items.is_empty() {
        return vec![orig.clone()];
    }
    // A block line has no text to split around — keep it and add the new blocks
    // after it as their own lines.
    if orig.content.is_block() {
        let mut result = Vec::with_capacity(block_items.len() + 1);
        result.push(orig.clone());
        result.extend(block_items);
        return result;
    }

    let text = orig.text();
    let mut split = col.min(text.len());
    while split > 0 && !text.is_char_boundary(split) {
        split -= 1;
    }
    let before = text[..split].to_string();
    let after = text[split..].to_string();

    let mut result = Vec::with_capacity(block_items.len() + 2);
    if before.is_empty() {
        // No leading text: the blocks lead. The original line (its id + metadata)
        // carries the trailing text; if there is none, the first block inherits
        // the original line's identity so the line stays stable.
        result.extend(block_items);
        if after.is_empty() {
            if let Some(first) = result.first_mut() {
                first.id = orig.id;
                first.marker = orig.marker;
                first.indent = orig.indent;
                first.start = orig.start;
                first.end = orig.end;
                first.available = orig.available;
                first.repeats = orig.repeats.clone();
                first.state = orig.state.clone();
                first.priority = orig.priority;
                first.external = orig.external.clone();
            }
        } else {
            let mut after_item = orig.clone();
            after_item.set_text(after);
            result.push(after_item);
        }
    } else {
        let mut before_item = orig.clone();
        before_item.set_text(before);
        result.push(before_item);
        result.extend(block_items);
        if !after.is_empty() {
            let mut after_item = Item::new("");
            after_item.indent = orig.indent;
            after_item.set_text(after);
            result.push(after_item);
        }
    }
    result
}

/// Splice a cut/copied run of `items` (whose first and last entries may be
/// *partial* line fragments — a selection that spanned text and a block) into
/// `current` at byte offset `col`. The leading fragment merges into the text
/// before the caret and the trailing fragment into the text after it, so a cut
/// immediately followed by a paste restores the original line structure.
///
/// Returns the replacement items plus `(cursor_index, cursor_col)` — the item
/// index (within the returned items) and column where the caret should land,
/// which is the end of the spliced content (the original selection end), before
/// any trailing remainder.
pub(in crate::scheme_editor) fn splice_items_into_line(
    current: &Item,
    col: usize,
    items: Vec<Item>,
) -> (Vec<Item>, usize, usize) {
    let text = current.text();
    let mut split = col.min(text.len());
    while split > 0 && !text.is_char_boundary(split) {
        split -= 1;
    }
    let before = text[..split].to_string();
    let after = text[split..].to_string();

    let mut result = items;
    // Merge `before` into the first item: a leading text fragment extends the
    // caret line (keeping its identity); a leading block keeps `before` as its
    // own line.
    if result.first().is_some_and(|item| item.content.is_text()) {
        let merged = format!("{before}{}", result[0].text());
        let mut first = current.clone();
        first.set_text(merged);
        result[0] = first;
    } else if !before.is_empty() {
        let mut before_item = current.clone();
        before_item.set_text(before);
        result.insert(0, before_item);
    }

    // The caret lands at the end of the last spliced item's own content.
    let cursor_index = result.len() - 1;
    let cursor_col = result[cursor_index].text().len();

    // Merge `after` into the last item, mirroring the leading rule.
    if result[cursor_index].content.is_text() {
        let mut tail = result[cursor_index].text();
        tail.push_str(&after);
        result[cursor_index].set_text(tail);
    } else if !after.is_empty() {
        result.push(Item::new(after));
    }

    (result, cursor_index, cursor_col)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::scheme_editor) struct TableMergeResult {
    pub(in crate::scheme_editor) target: ItemId,
    pub(in crate::scheme_editor) deleted: ItemId,
    pub(in crate::scheme_editor) target_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::scheme_editor) struct TableSplitResult {
    pub(in crate::scheme_editor) table: ItemId,
    pub(in crate::scheme_editor) table_index: usize,
}

pub(in crate::scheme_editor) fn merge_table_item_into(
    items: &mut Vec<Item>,
    target_index: usize,
    table_index: usize,
) -> Option<TableMergeResult> {
    if target_index == table_index
        || target_index >= items.len()
        || table_index >= items.len()
        || !item_has_block_object(&items[table_index])
        // Single-content: a block fills its whole line, so it can only absorb an
        // *empty* neighbour. Refuse to merge it onto a line that still has its
        // own content (which would silently drop that content).
        || !items[target_index].is_content_empty()
    {
        return None;
    }

    let table_item = items.remove(table_index);
    let target_index = if table_index < target_index {
        target_index.checked_sub(1)?
    } else {
        target_index
    };
    let deleted = table_item.id;
    let target = items.get_mut(target_index)?;
    append_content(target, &table_item);

    Some(TableMergeResult {
        target: target.id,
        deleted,
        target_index,
    })
}

/// Merge `source`'s content onto `target`. A line is single-content, so the
/// flat inline runs are concatenated and collapsed (a block wins over text),
/// matching the old "append the trailing content" join behavior.
fn append_content(target: &mut Item, source: &Item) {
    let mut combined = target.content.to_inlines();
    combined.extend(source.content.to_inlines());
    target.content = ItemContent::from_inlines(combined);
}

pub(in crate::scheme_editor) fn append_item_into_table(
    items: &mut Vec<Item>,
    table_index: usize,
    item_index: usize,
) -> Option<TableMergeResult> {
    if table_index == item_index
        || table_index >= items.len()
        || item_index >= items.len()
        || !item_has_block_object(&items[table_index])
        // Single-content: the block can only absorb an *empty* neighbour, never
        // a line that still carries its own text/image/table.
        || !items[item_index].is_content_empty()
    {
        return None;
    }

    let item = items.remove(item_index);
    let table_index = if item_index < table_index {
        table_index.checked_sub(1)?
    } else {
        table_index
    };
    let deleted = item.id;
    let target = items.get_mut(table_index)?;
    append_content(target, &item);

    Some(TableMergeResult {
        target: target.id,
        deleted,
        target_index: table_index,
    })
}

pub(in crate::scheme_editor) fn split_table_item_at_text_col(
    items: &mut Vec<Item>,
    item_index: usize,
    col: usize,
) -> Option<TableSplitResult> {
    let item = items.get(item_index)?.clone();
    if !item_has_block_object(&item) {
        return None;
    }

    let display = display_line_for_row(&EditorRow {
        item: item.clone(),
        path: if item.has_table() {
            RowPath::anchor()
        } else {
            RowPath::doc()
        },
    });
    if col > display.len() || !display.is_char_boundary(col) {
        return None;
    }

    if col == 0 {
        let mut blank = Item::new("");
        blank.indent = item.indent;
        items.insert(item_index, blank);
        return Some(TableSplitResult {
            table: item.id,
            table_index: item_index + 1,
        });
    }

    let mut before_item = item.clone();
    let mut blocks = block_inlines_for_item(&item, None).into_iter();
    before_item.content =
        ItemContent::from_inlines(content_from_block_line(&display[..col], &mut blocks));

    let mut table_item = Item::new("");
    table_item.indent = item.indent;
    table_item.marker = item.marker;
    table_item.content =
        ItemContent::from_inlines(content_from_block_line(&display[col..], &mut blocks));
    let table = table_item.id;

    items[item_index] = before_item;
    items.insert(item_index + 1, table_item);
    Some(TableSplitResult {
        table,
        table_index: item_index + 1,
    })
}
