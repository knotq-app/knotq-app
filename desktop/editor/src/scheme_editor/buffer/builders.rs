use super::*;

pub(in crate::scheme_editor) fn build_buffer(items: &[Item]) -> (String, Vec<EditorRow>) {
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

pub(in crate::scheme_editor) fn display_line_for_row(row: &EditorRow) -> String {
    if item_has_block_object(&row.item) {
        return clean_display_line_text(&item_inline_text_with_block_objects(&row.item));
    }
    clean_display_line_text(&row.item.text())
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
