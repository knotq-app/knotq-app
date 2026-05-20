use std::ops::Range;

use knotq_model::{Item, ItemId, ItemMarker};

#[derive(Clone, Debug)]
pub struct EditorRow {
    pub item_id: ItemId,
    pub item_index: usize,
    pub marker: ItemMarker,
    pub indent: u8,
    pub text_range: Range<usize>,
    pub annotation_height: f32,
    pub item: Item,
}

#[derive(Clone, Debug)]
pub struct EditorBuffer {
    pub text: String,
    pub rows: Vec<EditorRow>,
}

impl EditorBuffer {
    pub fn from_items(items: &[Item]) -> Self {
        let mut text = String::new();
        let mut rows = Vec::with_capacity(items.len());
        for (item_index, item) in items.iter().enumerate() {
            if item_index > 0 {
                text.push('\n');
            }
            let start = text.len();
            text.push_str(&clean_line_text(&item.text));
            let end = text.len();
            rows.push(EditorRow {
                item_id: item.id,
                item_index,
                marker: item.marker,
                indent: item.indent,
                text_range: start..end,
                annotation_height: 0.0,
                item: item.clone(),
            });
        }
        if items.is_empty() {
            rows.push(EditorRow {
                item_id: ItemId::new(),
                item_index: 0,
                marker: ItemMarker::Blank,
                indent: 0,
                text_range: 0..0,
                annotation_height: 0.0,
                item: Item::new(""),
            });
        }
        Self { text, rows }
    }
}

pub fn clean_line_text(text: &str) -> String {
    text.trim_start_matches([' ', '\t']).replace('\t', " ")
}

pub fn same_rows(a: &[EditorRow], b: &[EditorRow]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.item_id == b.item_id && same_item(&a.item, &b.item))
}

pub fn line_ranges(text: &str) -> Vec<Range<usize>> {
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

fn same_item(a: &Item, b: &Item) -> bool {
    a.id == b.id
        && a.text == b.text
        && a.media == b.media
        && a.marker == b.marker
        && a.indent == b.indent
        && a.start == b.start
        && a.end == b.end
        && a.available == b.available
        && a.repeats == b.repeats
        && a.priority == b.priority
        && a.state.len() == b.state.len()
        && a.state.iter().zip(&b.state).all(|(a, b)| {
            a.occurrence == b.occurrence
                && a.state.progress == b.state.progress
                && a.state.notification_offset_secs == b.state.notification_offset_secs
        })
}
