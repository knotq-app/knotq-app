use super::*;

pub(in crate::scheme_editor) fn build_buffer(items: &[Item]) -> (String, Vec<EditorRow>) {
    let mut rows = Vec::with_capacity(items.len());
    rebuild_rows_into(items, &mut rows);
    let mut text = String::new();
    write_display_text(&rows, &mut text);
    (text, rows)
}

/// Rebuild `rows` and `text` from `items`, keeping every row that has not
/// changed, and report whether anything did.
///
/// The editor rebuilt its whole buffer from the model on every keystroke, so a
/// 10,000-line scheme cloned 10,000 `Item`s and allocated ~30,000 strings —
/// 4.4ms per keypress — and then, on a local edit, found the result identical
/// to what it already had and threw it away. A row whose item is unchanged is
/// now left in place, and the display text is appended into the caller's
/// buffer instead of building a `String` per line and joining them.
///
/// The reuse decision is [`same_item`], so a reused row is by construction a
/// row a fresh build would have produced identically.
pub(in crate::scheme_editor) fn rebuild_rows_into(items: &[Item], rows: &mut Vec<EditorRow>) -> bool {
    let mut writer = RowWriter {
        rows,
        next: 0,
        changed: false,
    };

    for item in items {
        if let Some(table) = item.table() {
            let anchor = writer.next;
            writer.emit(item, RowPath::anchor());
            for (c, column) in table.columns.iter().enumerate() {
                writer.emit_owned(
                    header_item(column.id, &column.name),
                    RowPath::cell(anchor, HEADER_ROW, c, 0, 1),
                );
            }
            for (r, table_row) in table.rows.iter().enumerate() {
                for (c, cell) in table_row.cells.iter().enumerate() {
                    let cell_lines = cell.items.len().max(1);
                    for (sub, sub_item) in cell.items.iter().enumerate() {
                        writer.emit(sub_item, RowPath::cell(anchor, r, c, sub, cell_lines));
                    }
                }
            }
        } else {
            writer.emit(item, RowPath::doc());
        }
    }
    writer.finish()
}

/// The editor's flat text for `rows`, appended into `out`.
///
/// The text is a pure function of the rows, so it only needs rewriting when
/// they change. Written straight into the caller's buffer rather than building
/// a `String` per line and joining them.
pub(in crate::scheme_editor) fn write_display_text(rows: &[EditorRow], out: &mut String) {
    for (row, editor_row) in rows.iter().enumerate() {
        if row > 0 {
            out.push('\n');
        }
        push_display_line(out, editor_row);
    }
}

/// Writes rows into an existing vector, leaving unchanged ones alone.
struct RowWriter<'a> {
    rows: &'a mut Vec<EditorRow>,
    next: usize,
    changed: bool,
}

impl RowWriter<'_> {
    /// Emit a row for `item`, cloning it only if the row at this position is
    /// not already the same.
    fn emit(&mut self, item: &Item, path: RowPath) {
        if self.matches(item, path) {
            self.next += 1;
            return;
        }
        self.write(item.clone(), path);
    }

    /// Emit a row for an item the caller already had to build (a table header
    /// cell, which is synthesized rather than stored).
    fn emit_owned(&mut self, item: Item, path: RowPath) {
        if self.matches(&item, path) {
            self.next += 1;
            return;
        }
        self.write(item, path);
    }

    fn matches(&self, item: &Item, path: RowPath) -> bool {
        self.rows
            .get(self.next)
            .is_some_and(|existing| existing.path == path && same_item(&existing.item, item))
    }

    fn write(&mut self, item: Item, path: RowPath) {
        self.changed = true;
        let row = EditorRow { item, path };
        match self.rows.get_mut(self.next) {
            Some(slot) => *slot = row,
            None => self.rows.push(row),
        }
        self.next += 1;
    }

    /// Drop any rows the new item list no longer reaches.
    fn finish(self) -> bool {
        if self.rows.len() != self.next {
            self.rows.truncate(self.next);
            return true;
        }
        self.changed
    }
}

/// Append `row`'s display line. The allocating form is
/// [`display_line_for_row`]; a test pins them together.
fn push_display_line(out: &mut String, row: &EditorRow) {
    match &row.item.content {
        // A block line renders as one sentinel object char.
        ItemContent::Image(_) | ItemContent::Table(_) => out.push(TABLE_OBJECT_CHAR),
        ItemContent::Text { text } => push_clean_display_line(out, text),
    }
}

/// [`clean_display_line_text`] without the two intermediate `String`s: leading
/// spaces and tabs are dropped, and remaining tabs become single spaces.
fn push_clean_display_line(out: &mut String, text: &str) {
    let trimmed = text.trim_start_matches([' ', '\t']);
    if !trimmed.contains('\t') {
        out.push_str(trimmed);
        return;
    }
    for ch in trimmed.chars() {
        out.push(if ch == '\t' { ' ' } else { ch });
    }
}

pub(in crate::scheme_editor) fn display_line_for_row(row: &EditorRow) -> String {
    if item_has_block_object(&row.item) {
        return clean_display_line_text(&item_inline_text_with_block_objects(&row.item)).into_owned();
    }
    clean_display_line_text(&row.item.text()).into_owned()
}

fn item_inline_text_with_block_objects(item: &Item) -> String {
    // A line is single-content: a block (image/table) renders as one sentinel
    // object char; text renders as itself.
    match &item.content {
        ItemContent::Text { text } => text.clone(),
        ItemContent::Image(_) | ItemContent::Table(_) => TABLE_OBJECT_CHAR.to_string(),
    }
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

pub(in crate::scheme_editor) fn reconstruct_top_level(rows: &[EditorRow]) -> Vec<Item> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme_editor::buffer::same_rows;
    use knotq_model::{ItemMarker, Table};

    fn text_item(text: &str) -> Item {
        let mut item = Item::new("");
        item.set_text(text.to_string());
        item
    }

    fn table_item() -> Item {
        let mut table = Table::new(2, 2);
        table.columns[0].name = "Task".to_string();
        table.rows[0].cells[0].items[0].set_text("write it".to_string());
        table.rows[1].cells[1].items[0].marker = ItemMarker::Checkbox;
        let mut item = Item::new("");
        item.set_table(table);
        item
    }

    /// Rebuilding in place must land on exactly what a fresh build produces —
    /// rows AND text — for every edit shape. A reused row that a fresh build
    /// would have made differently is the editor showing stale content.
    #[test]
    fn rebuilding_in_place_matches_a_fresh_build() {
        let a = text_item("alpha");
        let b = text_item("beta");
        let c = text_item("gamma");
        let mut edited = a.clone();
        edited.set_text("alpha!".to_string());
        let mut remarked = b.clone();
        remarked.marker = ItemMarker::Checkbox;
        let mut indented = c.clone();
        indented.indent = 2;

        let sequences: Vec<Vec<Item>> = vec![
            vec![],
            vec![a.clone()],
            vec![a.clone(), b.clone(), c.clone()],
            // edit in place
            vec![edited.clone(), b.clone(), c.clone()],
            // metadata-only change
            vec![edited.clone(), remarked.clone(), c.clone()],
            vec![edited.clone(), remarked.clone(), indented.clone()],
            // insert at the front, the middle, the end
            vec![text_item("new"), edited.clone(), remarked.clone(), indented.clone()],
            vec![edited.clone(), text_item("mid"), remarked.clone()],
            vec![edited.clone(), remarked.clone(), text_item("tail")],
            // delete
            vec![remarked.clone()],
            // reorder
            vec![c.clone(), b.clone(), a.clone()],
            // tables expand into several rows
            vec![a.clone(), table_item(), c.clone()],
            vec![table_item()],
            vec![],
            vec![a.clone()],
        ];

        let mut rows = Vec::new();
        let mut text = String::new();
        for items in &sequences {
            let changed = rebuild_rows_into(items, &mut rows);
            text.clear();
            write_display_text(&rows, &mut text);

            let (fresh_text, fresh_rows) = build_buffer(items);
            assert!(
                same_rows(&rows, &fresh_rows),
                "rows diverged from a fresh build for {} items",
                items.len()
            );
            assert_eq!(rows.len(), fresh_rows.len());
            assert_eq!(text, fresh_text, "text diverged for {} items", items.len());
            // Paths carry table geometry that `same_rows` also checks, but be
            // explicit: an anchor index that drifted would misplace a cell.
            for (got, want) in rows.iter().zip(&fresh_rows) {
                assert_eq!(got.path, want.path);
            }
            let _ = changed;
        }
    }

    /// The reuse flag must be exact in both directions: a missed change leaves
    /// the editor stale, and a spurious one throws away the saving (and resets
    /// the caret, since the caller treats it as new content).
    #[test]
    fn the_changed_flag_reports_exactly_whether_anything_moved() {
        let a = text_item("alpha");
        let b = text_item("beta");
        let mut rows = Vec::new();

        assert!(rebuild_rows_into(&[a.clone(), b.clone()], &mut rows));
        assert!(!rebuild_rows_into(&[a.clone(), b.clone()], &mut rows));

        let mut edited = a.clone();
        edited.set_text("alpha!".to_string());
        assert!(rebuild_rows_into(&[edited.clone(), b.clone()], &mut rows));
        assert!(!rebuild_rows_into(&[edited.clone(), b.clone()], &mut rows));

        // Shorter, then longer.
        assert!(rebuild_rows_into(&[edited.clone()], &mut rows));
        assert!(!rebuild_rows_into(&[edited.clone()], &mut rows));
        assert!(rebuild_rows_into(&[edited.clone(), b.clone()], &mut rows));

        // A table's rows come and go with it.
        assert!(rebuild_rows_into(&[table_item()], &mut rows));
        assert!(rows.len() > 1);
    }

    /// The streaming display line must equal the allocating one, including the
    /// leading-whitespace trim and tab handling.
    #[test]
    fn the_streamed_display_line_matches_the_allocating_one() {
        let cases = [
            "plain",
            "   leading spaces",
            "\t\ttabs first",
            "inner\ttab",
            "  mixed \t both \t ways",
            "",
            "héllo 🙂",
        ];
        for case in cases {
            let row = EditorRow::doc(text_item(case));
            let mut streamed = String::new();
            push_display_line(&mut streamed, &row);
            assert_eq!(streamed, display_line_for_row(&row), "{case:?}");
        }

        // And for block lines, which render as the sentinel object char.
        let table = EditorRow::doc(table_item());
        let mut streamed = String::new();
        push_display_line(&mut streamed, &table);
        assert_eq!(streamed, display_line_for_row(&table));
    }
}
