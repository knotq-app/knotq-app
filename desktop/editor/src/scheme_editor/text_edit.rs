use super::*;

mod blocks;
mod sync;

pub(super) fn rebuild_tabled_rows_after_text_change(
    old_rows: &[EditorRow],
    new_lines: &[String],
    change: LineChange,
    selection_head: TextLocation,
) -> Vec<EditorRow> {
    let old_changed = change.old_suffix.saturating_sub(change.prefix);
    let new_changed = change.new_suffix.saturating_sub(change.prefix);
    let inserted_path = table_inserted_row_path(old_rows, change, selection_head);
    let inserted_style = table_inserted_row_style(old_rows, change, selection_head);

    let mut new_rows = Vec::with_capacity(new_lines.len());
    for (i, line) in new_lines.iter().enumerate().take(change.prefix) {
        let Some(old_row) = old_rows.get(i) else {
            continue;
        };
        let mut row = old_row.clone();
        set_row_text_from_buffer_line(&mut row, line);
        new_rows.push(row);
    }

    for offset in 0..new_changed {
        let line_index = change.prefix + offset;
        let Some(line) = new_lines.get(line_index) else {
            continue;
        };
        if offset < old_changed {
            if let Some(old_row) = old_rows.get(change.prefix + offset) {
                let mut row = old_row.clone();
                set_row_text_from_buffer_line(&mut row, line);
                new_rows.push(row);
                continue;
            }
        }

        new_rows.push(EditorRow {
            item: item_for_inserted_line(line.clone(), inserted_style),
            path: inserted_path,
        });
    }

    for i in change.old_suffix..old_rows.len() {
        let mut row = old_rows[i].clone();
        let new_index = change.new_suffix + (i - change.old_suffix);
        if let Some(line) = new_lines.get(new_index) {
            set_row_text_from_buffer_line(&mut row, line);
        }
        new_rows.push(row);
    }

    new_rows
}

fn set_row_text_from_buffer_line(row: &mut EditorRow, line: &str) {
    if row.path.is_table_anchor() {
        let table = row
            .item
            .table()
            .cloned()
            .unwrap_or_else(|| knotq_model::Table::new(1, 1));
        set_table_anchor_content_from_line(&mut row.item, line, table);
    } else if row.item.has_images() {
        set_item_content_from_block_line(&mut row.item, line, None);
    } else {
        row.item.set_text(clean_line_text(line));
    }
}

fn table_inserted_row_path(
    old_rows: &[EditorRow],
    change: LineChange,
    selection_head: TextLocation,
) -> RowPath {
    if let Some(row) = old_rows
        .get(selection_head.row)
        .filter(|row| row.path.is_cell())
    {
        return row.path;
    }

    [
        old_rows.get(change.prefix),
        change
            .prefix
            .checked_sub(1)
            .and_then(|index| old_rows.get(index)),
        old_rows.get(change.old_suffix),
    ]
    .into_iter()
    .flatten()
    .find_map(|row| row.path.is_cell().then_some(row.path))
    .unwrap_or_default()
}

fn table_inserted_row_style(
    old_rows: &[EditorRow],
    change: LineChange,
    selection_head: TextLocation,
) -> Option<InsertedLineStyle> {
    [
        old_rows.get(selection_head.row),
        old_rows.get(change.prefix),
        change
            .prefix
            .checked_sub(1)
            .and_then(|index| old_rows.get(index)),
        old_rows.get(change.old_suffix),
    ]
    .into_iter()
    .flatten()
    .find(|row| !row.path.is_table_anchor())
    .map(|row| InsertedLineStyle::from_item(&row.item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::Table;

    fn table_item(rows: usize, cols: usize) -> Item {
        let mut item = Item::new("");
        item.set_table(Table::new(rows, cols));
        item
    }

    fn text_lines(text: &str) -> Vec<String> {
        text.split('\n').map(clean_line_text).collect()
    }

    /// Index of the first body cell row (after the anchor and the header row).
    fn first_body_cell(rows: &[EditorRow]) -> usize {
        rows.iter()
            .position(|row| row.path.is_cell() && !row.path.is_header_cell())
            .expect("table has a body cell")
    }

    #[test]
    fn tabled_text_replacement_preserves_the_edited_cell_path() {
        let item = table_item(2, 2);
        let (old_text, old_rows) = build_buffer(&[item]);
        let body0 = first_body_cell(&old_rows);
        let mut new_lines = text_lines(&old_text);
        new_lines[body0] = "Alpha".to_string();
        let old_lines = text_lines(&old_text);
        let old_refs: Vec<&str> = old_lines.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new_lines.iter().map(String::as_str).collect();
        let change = line_change(&old_refs, &new_refs);

        let rows = rebuild_tabled_rows_after_text_change(
            &old_rows,
            &new_lines,
            change,
            TextLocation { row: body0, col: 0 },
        );
        assert!(rows[body0].path.is_cell());
        assert_eq!((rows[body0].path.r, rows[body0].path.c), (0, 0));

        let top = reconstruct_top_level(&rows);
        assert_eq!(top.len(), 1);
        let table = top[0].table().unwrap();
        assert_eq!(table.cell(0, 0).unwrap().items[0].text(), "Alpha");
        assert_eq!(table.cell(0, 1).unwrap().items[0].text(), "");
    }

    #[test]
    fn tabled_line_insertion_uses_the_active_cell_path() {
        let mut item = table_item(2, 2);
        item.table_mut().unwrap().cell_mut(0, 0).unwrap().items[0].set_text("Alpha".to_string());
        let (old_text, old_rows) = build_buffer(&[item]);
        let body0 = first_body_cell(&old_rows);
        let mut new_lines = text_lines(&old_text);
        new_lines.insert(body0 + 1, "Second line".to_string());
        let old_lines = text_lines(&old_text);
        let old_refs: Vec<&str> = old_lines.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new_lines.iter().map(String::as_str).collect();
        let change = line_change(&old_refs, &new_refs);

        let rows = rebuild_tabled_rows_after_text_change(
            &old_rows,
            &new_lines,
            change,
            TextLocation { row: body0, col: 5 },
        );
        let top = reconstruct_top_level(&rows);
        let table = top[0].table().unwrap();

        assert_eq!(table.cell(0, 0).unwrap().items.len(), 2);
        assert_eq!(table.cell(0, 0).unwrap().items[0].text(), "Alpha");
        assert_eq!(table.cell(0, 0).unwrap().items[1].text(), "Second line");
        assert_eq!(table.cell(0, 1).unwrap().items[0].text(), "");
    }
}
