use std::ops::Range;

use knotq_model::{ColumnId, Inline, Item, ItemId, ItemMarker, Table};
use uuid::Uuid;

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
        .map(|row| display_line(&row.item.text()))
        .collect::<Vec<_>>()
        .join("\n");
    (text, rows)
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
                replace_or_append_table(&mut item, table);
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

fn replace_or_append_table(item: &mut Item, table: Table) {
    if let Some(slot) = item
        .content
        .iter_mut()
        .find(|inline| matches!(inline, Inline::Table(_)))
    {
        *slot = Inline::Table(table);
    } else {
        item.content.push(Inline::Table(table));
    }
}

pub(super) fn rows_have_table(rows: &[EditorRow]) -> bool {
    rows.iter().any(|row| row.path.is_table_anchor())
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
    clean_line_text(text)
}

pub(super) fn clean_line_text(text: &str) -> String {
    text.trim_start_matches([' ', '\t']).replace('\t', " ")
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

        assert_eq!(
            text.lines().take(3).collect::<Vec<_>>(),
            ["", "Column 1", "Column 2"]
        );
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
}
