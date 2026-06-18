use std::ops::Range;

use knotq_model::{ColumnId, Inline, Item, ItemId, ItemMarker, Table};
use uuid::Uuid;

pub(super) const TABLE_OBJECT_CHAR: char = '\u{fffc}';
pub(super) const TABLE_OBJECT_LEN: usize = TABLE_OBJECT_CHAR.len_utf8();

/// Sentinel table-row index for a header cell. Header cells map to a column's
/// *name* rather than a body row, so they live "above" body row 0 — and
/// `HEADER_ROW as isize == -1` makes vertical navigation fall out naturally.
pub(super) const HEADER_ROW: usize = usize::MAX;

/// Where a buffer row lives in the document tree. The editor keeps one flat
/// text buffer so the ordinary text pipeline can edit table cells too.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum RowKind {
    #[default]
    Doc,
    TableAnchor,
    Cell,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct RowPath {
    pub(super) kind: RowKind,
    /// Buffer index of the owning table's anchor row. Rebuilt on every
    /// `build_buffer`, so it is valid only against the current row vector.
    pub(super) anchor: usize,
    pub(super) r: usize,
    pub(super) c: usize,
    pub(super) sub: usize,
    pub(super) cell_lines: usize,
}

impl RowPath {
    fn doc() -> Self {
        Self::default()
    }

    fn anchor() -> Self {
        Self {
            kind: RowKind::TableAnchor,
            ..Default::default()
        }
    }

    fn cell(anchor: usize, r: usize, c: usize, sub: usize, cell_lines: usize) -> Self {
        Self {
            kind: RowKind::Cell,
            anchor,
            r,
            c,
            sub,
            cell_lines,
        }
    }

    pub(super) fn is_cell(&self) -> bool {
        self.kind == RowKind::Cell
    }

    /// A header cell is a cell whose row index is the [`HEADER_ROW`] sentinel.
    pub(super) fn is_header_cell(&self) -> bool {
        self.kind == RowKind::Cell && self.r == HEADER_ROW
    }

    pub(super) fn is_doc(&self) -> bool {
        self.kind == RowKind::Doc
    }

    pub(super) fn is_table_anchor(&self) -> bool {
        self.kind == RowKind::TableAnchor
    }

    pub(super) fn is_first_in_cell(&self) -> bool {
        self.kind == RowKind::Cell && self.sub == 0
    }

    pub(super) fn is_last_in_cell(&self) -> bool {
        self.kind == RowKind::Cell && self.sub + 1 >= self.cell_lines
    }
}

#[derive(Clone)]
pub(super) struct EditorRow {
    pub(super) item: Item,
    pub(super) path: RowPath,
}

impl EditorRow {
    pub(super) fn doc(item: Item) -> Self {
        Self {
            item,
            path: RowPath::doc(),
        }
    }
}

pub(super) fn build_buffer(items: &[Item]) -> (String, Vec<EditorRow>) {
    let mut rows = Vec::with_capacity(items.len());
    for item in items {
        if let Some(table) = item.table() {
            let anchor = rows.len();
            rows.push(EditorRow {
                item: item.clone(),
                path: RowPath::anchor(),
            });
            for (c, column) in table.columns.iter().enumerate() {
                rows.push(EditorRow {
                    item: header_item(column.id, &column.name),
                    path: RowPath::cell(anchor, HEADER_ROW, c, 0, 1),
                });
            }
            for (r, table_row) in table.rows.iter().enumerate() {
                for (c, cell) in table_row.cells.iter().enumerate() {
                    let cell_lines = cell.items.len().max(1);
                    for (sub, sub_item) in cell.items.iter().enumerate() {
                        rows.push(EditorRow {
                            item: sub_item.clone(),
                            path: RowPath::cell(anchor, r, c, sub, cell_lines),
                        });
                    }
                }
            }
        } else {
            rows.push(EditorRow::doc(item.clone()));
        }
    }
    let text = rows
        .iter()
        .map(display_line_for_row)
        .collect::<Vec<_>>()
        .join("\n");
    (text, rows)
}

pub(super) fn display_line_for_row(row: &EditorRow) -> String {
    if item_has_block_object(&row.item) {
        return display_line(&item_inline_text_with_block_objects(&row.item));
    }
    display_line(&row.item.text())
}

fn item_inline_text_with_block_objects(item: &Item) -> String {
    let mut line = String::new();
    for inline in &item.content {
        match inline {
            Inline::Text { text } => line.push_str(text),
            Inline::Table(_) | Inline::Image(_) => line.push(TABLE_OBJECT_CHAR),
        }
    }
    line
}

fn header_item(column: ColumnId, name: &str) -> Item {
    let mut item = Item::new(name.to_string());
    item.id = header_item_id(column);
    item
}

fn header_item_id(column: ColumnId) -> ItemId {
    let mut bytes = column.0.into_bytes();
    bytes[0] ^= 0x80;
    ItemId(Uuid::from_bytes(bytes))
}

pub(super) fn reconstruct_top_level(rows: &[EditorRow]) -> Vec<Item> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        match rows[i].path.kind {
            RowKind::Doc => {
                out.push(rows[i].item.clone());
                i += 1;
            }
            RowKind::TableAnchor => {
                let mut item = rows[i].item.clone();
                let mut table = item.table().cloned().unwrap_or_else(|| Table::new(1, 1));
                let anchor_line = display_line_for_row(&rows[i]);
                for table_row in &mut table.rows {
                    for cell in &mut table_row.cells {
                        cell.items.clear();
                    }
                }
                i += 1;
                while i < rows.len() && rows[i].path.is_cell() {
                    let path = rows[i].path;
                    if path.is_header_cell() {
                        if let Some(column) = table.columns.get_mut(path.c) {
                            column.name = rows[i].item.text();
                        }
                    } else if let Some(table_row) = table.rows.get_mut(path.r) {
                        if let Some(cell) = table_row.cells.get_mut(path.c) {
                            cell.items.push(rows[i].item.clone());
                        }
                    }
                    i += 1;
                }
                table.normalize();
                set_table_anchor_content_from_line(&mut item, &anchor_line, table);
                out.push(item);
            }
            RowKind::Cell => {
                // Preserve unexpected stray cell rows as plain document lines.
                out.push(rows[i].item.clone());
                i += 1;
            }
        }
    }
    out
}

pub(super) fn set_table_anchor_content_from_line(item: &mut Item, line: &str, table: Table) {
    let mut line = clean_display_line_text(line);
    if table_object_range(&line).is_none() {
        line.push(TABLE_OBJECT_CHAR);
    }
    set_item_content_from_block_line(item, &line, Some(table));
}

pub(super) fn set_item_content_from_block_line(item: &mut Item, line: &str, table: Option<Table>) {
    let line = clean_display_line_text(line);
    let mut blocks = block_inlines_for_item(item, table).into_iter();
    item.content = content_from_block_line(&line, &mut blocks);
}

fn block_inlines_for_item(item: &Item, table: Option<Table>) -> Vec<Inline> {
    let mut used_table_override = false;
    let mut blocks = item
        .content
        .iter()
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

pub(super) fn item_has_block_object(item: &Item) -> bool {
    item.has_table() || item.has_images()
}

pub(super) fn rows_have_block_object(rows: &[EditorRow]) -> bool {
    rows.iter().any(|row| item_has_block_object(&row.item))
}

pub(super) fn flat_row_for_top_level_index(rows: &[EditorRow], top_level_index: usize) -> usize {
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

pub(super) fn top_level_index_for_flat_row(rows: &[EditorRow], target_row: usize) -> Option<usize> {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TableMergeResult {
    pub(super) target: ItemId,
    pub(super) deleted: ItemId,
    pub(super) target_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TableSplitResult {
    pub(super) table: ItemId,
    pub(super) table_index: usize,
}

pub(super) fn merge_table_item_into(
    items: &mut Vec<Item>,
    target_index: usize,
    table_index: usize,
) -> Option<TableMergeResult> {
    if target_index == table_index
        || target_index >= items.len()
        || table_index >= items.len()
        || !item_has_block_object(&items[table_index])
        || (items[table_index].has_table() && items[target_index].has_table())
    {
        return None;
    }

    let mut table_item = items.remove(table_index);
    let target_index = if table_index < target_index {
        target_index.checked_sub(1)?
    } else {
        target_index
    };
    let deleted = table_item.id;
    let target = items.get_mut(target_index)?;
    target.content.append(&mut table_item.content);

    Some(TableMergeResult {
        target: target.id,
        deleted,
        target_index,
    })
}

pub(super) fn append_item_into_table(
    items: &mut Vec<Item>,
    table_index: usize,
    item_index: usize,
) -> Option<TableMergeResult> {
    if table_index == item_index
        || table_index >= items.len()
        || item_index >= items.len()
        || !item_has_block_object(&items[table_index])
        || (items[table_index].has_table() && items[item_index].has_table())
    {
        return None;
    }

    let mut item = items.remove(item_index);
    let table_index = if item_index < table_index {
        table_index.checked_sub(1)?
    } else {
        table_index
    };
    let deleted = item.id;
    let target = items.get_mut(table_index)?;
    target.content.append(&mut item.content);

    Some(TableMergeResult {
        target: target.id,
        deleted,
        target_index: table_index,
    })
}

pub(super) fn split_table_item_at_text_col(
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
    before_item.content = content_from_block_line(&display[..col], &mut blocks);

    let mut table_item = Item::new("");
    table_item.indent = item.indent;
    table_item.marker = item.marker;
    table_item.content = content_from_block_line(&display[col..], &mut blocks);
    let table = table_item.id;

    items[item_index] = before_item;
    items.insert(item_index + 1, table_item);
    Some(TableSplitResult {
        table,
        table_index: item_index + 1,
    })
}

pub(super) fn same_rows(a: &[EditorRow], b: &[EditorRow]) -> bool {
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

fn display_line(text: &str) -> String {
    clean_display_line_text(text)
}

pub(super) fn clean_display_line_text(text: &str) -> String {
    text.trim_start_matches([' ', '\t']).replace('\t', " ")
}

pub(super) fn clean_line_text(text: &str) -> String {
    line_without_table_object(&clean_display_line_text(text))
}

pub(super) fn line_without_table_object(line: &str) -> String {
    line.replace(TABLE_OBJECT_CHAR, "")
}

pub(super) fn table_object_range(line: &str) -> Option<Range<usize>> {
    line.find(TABLE_OBJECT_CHAR)
        .map(|start| start..start + TABLE_OBJECT_LEN)
}

pub(super) fn block_object_ranges(line: &str) -> Vec<Range<usize>> {
    line.match_indices(TABLE_OBJECT_CHAR)
        .map(|(start, _)| start..start + TABLE_OBJECT_LEN)
        .collect()
}

pub(super) fn block_suffix_range(line: &str) -> Option<Range<usize>> {
    let object = block_object_ranges(line).into_iter().last()?;
    (object.end < line.len()).then_some(object.end..line.len())
}

pub(super) fn item_is_done(item: &Item) -> bool {
    item.marker == ItemMarker::Checkbox
        && item.repeats.is_none()
        && !item.state.is_empty()
        && item.state.iter().all(|state| state.state.is_done())
}

pub(super) fn item_is_partial(item: &Item) -> bool {
    item.marker == ItemMarker::Checkbox
        && (item.repeats.is_some() || item.state.iter().any(|state| state.state.is_done()))
        && !item_is_done(item)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LineChange {
    pub(super) prefix: usize,
    pub(super) old_suffix: usize,
    pub(super) new_suffix: usize,
}

pub(super) fn line_change(old_lines: &[&str], new_lines: &[&str]) -> LineChange {
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }

    let mut old_suffix = old_lines.len();
    let mut new_suffix = new_lines.len();
    while old_suffix > prefix
        && new_suffix > prefix
        && old_lines[old_suffix - 1] == new_lines[new_suffix - 1]
    {
        old_suffix -= 1;
        new_suffix -= 1;
    }

    LineChange {
        prefix,
        old_suffix,
        new_suffix,
    }
}

pub(super) fn line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            ranges.push(start..idx);
            start = idx + ch.len_utf8();
        }
    }
    ranges.push(start..text.len());
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::{ImageAssetFormat, ImageInline, Table};
    use uuid::Uuid;

    #[test]
    fn display_lines_keep_hard_indent_out_of_text() {
        let item = Item::new("child").with_indent(2);
        let (text, rows) = build_buffer(&[item]);
        assert_eq!(text, "child");
        assert_eq!(rows[0].item.indent, 2);
        assert_eq!(clean_line_text("\t    child"), "child");
    }

    #[test]
    fn line_change_finds_middle_replacement() {
        let old = ["a", "b", "c", "d"];
        let new = ["a", "x", "y", "d"];
        assert_eq!(
            line_change(&old, &new),
            LineChange {
                prefix: 1,
                old_suffix: 3,
                new_suffix: 3,
            }
        );
    }

    #[test]
    fn empty_text_still_has_one_logical_line() {
        assert_eq!(line_ranges(""), vec![0..0]);
        assert_eq!(line_ranges("a\n"), vec![0..1, 2..2]);
    }

    #[test]
    fn table_headers_are_editable_buffer_rows() {
        let mut item = Item::new("");
        item.content.push(Inline::Table(Table::new(1, 2)));

        let (text, rows) = build_buffer(&[item.clone()]);
        let (_, rebuilt_rows) = build_buffer(&[item]);

        let lines = text.lines().take(3).collect::<Vec<_>>();
        assert_eq!(lines[0], TABLE_OBJECT_CHAR.to_string());
        assert_eq!(lines[1], "Column 1");
        assert_eq!(lines[2], "Column 2");
        assert_eq!(table_object_range(lines[0]), Some(0..TABLE_OBJECT_LEN));
        assert_eq!(clean_line_text(lines[0]), "");
        assert!(rows[1].path.is_header_cell());
        assert_eq!((rows[1].path.r, rows[1].path.c), (HEADER_ROW, 0));
        assert!(rows[2].path.is_header_cell());
        assert_eq!((rows[2].path.r, rows[2].path.c), (HEADER_ROW, 1));
        assert_eq!(rows[1].item.id, rebuilt_rows[1].item.id);
        assert_ne!(rows[1].item.id, rows[2].item.id);
    }

    #[test]
    fn edited_header_rows_reconstruct_to_column_names() {
        let mut item = Item::new("");
        item.content.push(Inline::Table(Table::new(1, 2)));
        let (_, mut rows) = build_buffer(&[item]);

        rows[1].item.set_text("Project".to_string());
        rows[2].item.set_text("Owner".to_string());
        let top = reconstruct_top_level(&rows);
        let table = top[0].table().expect("table remains inline");

        assert_eq!(table.columns[0].name, "Project");
        assert_eq!(table.columns[1].name, "Owner");
    }

    #[test]
    fn table_anchor_buffer_preserves_inline_order() {
        let mut item = Item::new("");
        item.content.push(Inline::text("Before"));
        item.content.push(Inline::Table(Table::new(1, 1)));
        item.content.push(Inline::text("After"));

        let (text, rows) = build_buffer(&[item]);
        let line = text.lines().next().unwrap();
        assert_eq!(line, format!("Before{}After", TABLE_OBJECT_CHAR));

        let rebuilt = reconstruct_top_level(&rows);
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].content.len(), 3);
        assert!(matches!(rebuilt[0].content[0], Inline::Text { .. }));
        assert!(matches!(rebuilt[0].content[1], Inline::Table(_)));
        assert!(matches!(rebuilt[0].content[2], Inline::Text { .. }));
        assert_eq!(rebuilt[0].text(), "BeforeAfter");
    }

    #[test]
    fn table_merge_keeps_table_inline_as_a_single_object() {
        let before = Item::new("Before");
        let mut table = Item::new("");
        table.content.push(Inline::Table(Table::new(1, 2)));
        let after = Item::new("After");
        let table_id = table.id;

        let mut top = vec![before.clone(), table, after.clone()];
        let result = merge_table_item_into(&mut top, 0, 1).expect("table merges into text item");

        assert_eq!(result.target, before.id);
        assert_eq!(result.deleted, table_id);
        assert_eq!(result.target_index, 0);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].text(), "Before");
        assert!(top[0].has_table());
        assert_eq!(top[1].id, after.id);

        let (text, rows) = build_buffer(&top);
        let lines = text.lines().take(3).collect::<Vec<_>>();
        assert_eq!(lines[0], format!("Before{}", TABLE_OBJECT_CHAR));
        assert_eq!(lines[1], "Column 1");
        assert_eq!(lines[2], "Column 2");
        assert_eq!(
            table_object_range(lines[0]),
            Some("Before".len().."Before".len() + TABLE_OBJECT_LEN)
        );
        assert!(rows[0].path.is_table_anchor());
        assert!(rows[1].path.is_header_cell());
        assert!(rows[2].path.is_header_cell());
        let rebuilt = reconstruct_top_level(&rows);
        assert_eq!(rebuilt.len(), 2);
        assert_eq!(rebuilt[0].text(), "Before");
        assert!(rebuilt[0].has_table());
        assert_eq!(rebuilt[1].text(), "After");
    }

    #[test]
    fn table_merge_does_not_create_multiple_tables_on_one_item() {
        let mut first = Item::new("First");
        first.content.push(Inline::Table(Table::new(1, 1)));
        let mut second = Item::new("Second");
        second.content.push(Inline::Table(Table::new(1, 1)));
        let mut top = vec![first, second];

        assert!(merge_table_item_into(&mut top, 0, 1).is_none());
        assert_eq!(top.len(), 2);
        assert!(top[0].has_table());
        assert!(top[1].has_table());
    }

    #[test]
    fn table_backward_merge_appends_following_text_after_table() {
        let mut table = Item::new("");
        table.content.push(Inline::Table(Table::new(1, 1)));
        let table_id = table.id;
        let suffix = Item::new("After");
        let suffix_id = suffix.id;
        let mut top = vec![table, suffix];

        let result = append_item_into_table(&mut top, 0, 1).expect("suffix merges into table item");

        assert_eq!(result.target, table_id);
        assert_eq!(result.deleted, suffix_id);
        assert_eq!(result.target_index, 0);
        assert_eq!(top.len(), 1);
        assert!(matches!(top[0].content[0], Inline::Table(_)));
        assert!(matches!(&top[0].content[1], Inline::Text { text } if text == "After"));
    }

    #[test]
    fn table_split_moves_table_to_new_item_after_text() {
        let mut item = Item::new("BeforeAfter");
        item.indent = 2;
        item.content.push(Inline::Table(Table::new(1, 1)));
        let mut top = vec![item.clone()];

        let result =
            split_table_item_at_text_col(&mut top, 0, 6).expect("table splits from text item");

        assert_eq!(top.len(), 2);
        assert_eq!(top[0].id, item.id);
        assert_eq!(top[0].text(), "Before");
        assert!(!top[0].has_table());
        assert_eq!(top[1].id, result.table);
        assert_eq!(top[1].text(), "After");
        assert_eq!(top[1].indent, 2);
        assert!(top[1].has_table());
        assert_eq!(result.table_index, 1);
    }

    #[test]
    fn table_split_keeps_suffix_after_table() {
        let mut item = Item::new("");
        item.indent = 2;
        item.content.push(Inline::text("Before"));
        item.content.push(Inline::Table(Table::new(1, 1)));
        item.content.push(Inline::text("After"));
        let mut top = vec![item.clone()];

        let result =
            split_table_item_at_text_col(&mut top, 0, "Before".len()).expect("table splits");

        assert_eq!(top.len(), 2);
        assert_eq!(top[0].id, item.id);
        assert_eq!(top[0].text(), "Before");
        assert!(!top[0].has_table());
        assert_eq!(top[1].id, result.table);
        assert_eq!(top[1].text(), "After");
        assert_eq!(top[1].indent, 2);
        assert!(matches!(top[1].content[0], Inline::Table(_)));
        assert!(matches!(top[1].content[1], Inline::Text { .. }));
    }

    #[test]
    fn table_split_at_start_inserts_blank_before_existing_table() {
        let mut item = Item::new("");
        item.indent = 1;
        item.content.push(Inline::Table(Table::new(1, 1)));
        let table_id = item.id;
        let mut top = vec![item];

        let result =
            split_table_item_at_text_col(&mut top, 0, 0).expect("blank inserts before table");

        assert_eq!(top.len(), 2);
        assert_eq!(top[0].text(), "");
        assert!(!top[0].has_table());
        assert_eq!(top[0].indent, 1);
        assert_eq!(top[1].id, table_id);
        assert!(top[1].has_table());
        assert_eq!(result.table, table_id);
        assert_eq!(result.table_index, 1);
    }

    #[test]
    fn image_buffer_uses_object_char_and_preserves_order() {
        let mut item = Item::new("");
        item.content.push(Inline::text("Before"));
        item.content.push(Inline::Image(test_image()));
        item.content.push(Inline::text("After"));

        let (text, rows) = build_buffer(&[item]);
        assert_eq!(text, format!("Before{}After", TABLE_OBJECT_CHAR));
        assert_eq!(
            block_object_ranges(&text),
            vec!["Before".len().."Before".len() + TABLE_OBJECT_LEN]
        );

        let rebuilt = reconstruct_top_level(&rows);
        assert_eq!(rebuilt.len(), 1);
        assert!(matches!(rebuilt[0].content[0], Inline::Text { .. }));
        assert!(matches!(rebuilt[0].content[1], Inline::Image(_)));
        assert!(matches!(rebuilt[0].content[2], Inline::Text { .. }));
        assert_eq!(rebuilt[0].text(), "BeforeAfter");
    }

    #[test]
    fn image_backward_merge_appends_following_text_after_image() {
        let mut image = Item::new("");
        image.content.push(Inline::Image(test_image()));
        let image_id = image.id;
        let suffix = Item::new("After");
        let suffix_id = suffix.id;
        let mut top = vec![image, suffix];

        let result = append_item_into_table(&mut top, 0, 1).expect("suffix merges into image item");

        assert_eq!(result.target, image_id);
        assert_eq!(result.deleted, suffix_id);
        assert_eq!(result.target_index, 0);
        assert_eq!(top.len(), 1);
        assert!(matches!(top[0].content[0], Inline::Image(_)));
        assert!(matches!(&top[0].content[1], Inline::Text { text } if text == "After"));
    }

    #[test]
    fn image_split_moves_image_to_new_item_after_text() {
        let mut item = Item::new("");
        item.indent = 2;
        item.content.push(Inline::text("Before"));
        item.content.push(Inline::Image(test_image()));
        item.content.push(Inline::text("After"));
        let mut top = vec![item.clone()];

        let result =
            split_table_item_at_text_col(&mut top, 0, "Before".len()).expect("image splits");

        assert_eq!(top.len(), 2);
        assert_eq!(top[0].id, item.id);
        assert_eq!(top[0].text(), "Before");
        assert!(!top[0].has_images());
        assert_eq!(top[1].id, result.table);
        assert_eq!(top[1].text(), "After");
        assert_eq!(top[1].indent, 2);
        assert!(matches!(top[1].content[0], Inline::Image(_)));
        assert!(matches!(top[1].content[1], Inline::Text { .. }));
    }

    #[test]
    fn row_equality_tracks_done_state() {
        let open = Item::new("task");
        let done = Item::new("task").done();
        assert!(!same_rows(
            &[EditorRow::doc(open)],
            &[EditorRow::doc(done.clone())]
        ));
        assert!(item_is_done(&done));
    }

    #[test]
    fn row_equality_tracks_date_metadata() {
        let mut base = Item::new("task");
        base.marker = ItemMarker::Checkbox;
        let mut dated = Item::new("task").with_end(chrono::Utc::now());
        dated.marker = ItemMarker::Checkbox;

        assert!(!same_rows(
            &[EditorRow::doc(base)],
            &[EditorRow::doc(dated)]
        ));
    }

    #[test]
    fn row_equality_tracks_media_metadata() {
        let base = Item::new("image");
        let mut with_media = Item::new("image");
        with_media.push_image(ImageInline {
            asset: Uuid::new_v4(),
            format: ImageAssetFormat::Png,
            width: Some(32),
            height: Some(24),
        });

        assert!(!same_rows(
            &[EditorRow::doc(base)],
            &[EditorRow::doc(with_media)]
        ));
    }

    fn test_image() -> ImageInline {
        ImageInline {
            asset: Uuid::nil(),
            format: ImageAssetFormat::Png,
            width: Some(320),
            height: Some(200),
        }
    }
}
