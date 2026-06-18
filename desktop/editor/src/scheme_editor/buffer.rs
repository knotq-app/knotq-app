use std::ops::Range;

use knotq_model::{Item, ItemMarker};

#[derive(Clone)]
pub(super) struct EditorRow {
    pub(super) item: Item,
}

pub(super) fn build_buffer(items: &[Item]) -> (String, Vec<EditorRow>) {
    let text = items
        .iter()
        .map(|item| display_line(&item.text()))
        .collect::<Vec<_>>()
        .join("\n");
    let rows = items
        .iter()
        .cloned()
        .map(|item| EditorRow { item })
        .collect();
    (text, rows)
}

pub(super) fn same_rows(a: &[EditorRow], b: &[EditorRow]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.item.id == b.item.id
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
    use knotq_model::{ImageAssetFormat, ImageInline};
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
    fn row_equality_tracks_done_state() {
        let open = Item::new("task");
        let done = Item::new("task").done();
        assert!(!same_rows(
            &[EditorRow { item: open }],
            &[EditorRow { item: done.clone() }]
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
            &[EditorRow { item: base }],
            &[EditorRow { item: dated }]
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
            &[EditorRow { item: base }],
            &[EditorRow { item: with_media }]
        ));
    }
}
