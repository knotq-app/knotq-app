use knotq_model::{
    ImageAssetFormat, ImageInline, Inline, Item, ItemContent, SchemeId, Table, TableCell,
};
use knotq_sync::WorkspaceCrdtChangeSet;
use uuid::Uuid;

use super::{DeviceKey, Harness};

pub fn item_with_content(content: Vec<Inline>) -> Item {
    let mut item = Item::new("");
    item.content = ItemContent::from_inlines(content);
    item
}

pub fn set_item_content(
    h: &mut Harness,
    device: DeviceKey,
    scheme: SchemeId,
    index: usize,
    content: Vec<Inline>,
) {
    let test_device = h.device_mut_for_surgery(device);
    test_device.scheme_mut_pub(scheme).items[index].content = ItemContent::from_inlines(content);
    test_device.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme));
}

pub fn replace_scheme_items(h: &mut Harness, key: DeviceKey, scheme: SchemeId, items: Vec<Item>) {
    let device = h.device_mut_for_surgery(key);
    device.scheme_mut_pub(scheme).items = items;
    device.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme));
}

pub fn item_content(h: &Harness, device: DeviceKey, scheme: SchemeId, index: usize) -> Vec<Inline> {
    h.device(device).workspace.schemes[&scheme].items[index]
        .content
        .to_inlines()
}

pub fn table_with_cells(rows: &[&[&str]]) -> Table {
    let row_count = rows.len().max(1);
    let column_count = rows.first().map(|row| row.len()).unwrap_or(1).max(1);
    let mut table = Table::new(row_count, column_count);
    for (row_index, row) in rows.iter().enumerate() {
        for (column_index, text) in row.iter().enumerate() {
            table.rows[row_index].cells[column_index] = TableCell::with_text(*text);
        }
    }
    table
}

pub fn first_table(content: &[Inline]) -> Option<&Table> {
    content.iter().find_map(|inline| match inline {
        Inline::Table(table) => Some(table),
        _ => None,
    })
}

pub fn table_cell_texts(table: &Table) -> Vec<Vec<String>> {
    table
        .rows
        .iter()
        .map(|row| row.cells.iter().map(TableCell::summary_text).collect())
        .collect()
}

pub fn image_ref(width: u32, height: u32) -> ImageInline {
    ImageInline {
        asset: Uuid::new_v4(),
        format: ImageAssetFormat::Png,
        width: Some(width),
        height: Some(height),
    }
}

pub fn image_name(image: ImageInline) -> String {
    format!("{}.{}", image.asset, image.format.extension())
}

pub fn patterned_bytes(len: usize, modulo: u32) -> Vec<u8> {
    (0..len).map(|i| (i as u32 % modulo) as u8).collect()
}
