use std::ops::Range;

use knotq_model::{Item, ItemMarker};

use super::{TABLE_OBJECT_CHAR, TABLE_OBJECT_LEN};

pub(in crate::scheme_editor) fn clean_display_line_text(text: &str) -> String {
    text.trim_start_matches([' ', '\t']).replace('\t', " ")
}

pub(in crate::scheme_editor) fn clean_line_text(text: &str) -> String {
    line_without_table_object(&clean_display_line_text(text))
}

pub(in crate::scheme_editor) fn line_without_table_object(line: &str) -> String {
    line.replace(TABLE_OBJECT_CHAR, "")
}

pub(in crate::scheme_editor) fn table_object_range(line: &str) -> Option<Range<usize>> {
    line.find(TABLE_OBJECT_CHAR)
        .map(|start| start..start + TABLE_OBJECT_LEN)
}

pub(in crate::scheme_editor) fn block_object_ranges(line: &str) -> Vec<Range<usize>> {
    line.match_indices(TABLE_OBJECT_CHAR)
        .map(|(start, _)| start..start + TABLE_OBJECT_LEN)
        .collect()
}

pub(in crate::scheme_editor) fn block_suffix_range(line: &str) -> Option<Range<usize>> {
    let object = block_object_ranges(line).into_iter().last()?;
    (object.end < line.len()).then_some(object.end..line.len())
}

pub(in crate::scheme_editor) fn item_is_done(item: &Item) -> bool {
    item.marker == ItemMarker::Checkbox
        && item.repeats.is_none()
        && !item.state.is_empty()
        && item.state.iter().all(|state| state.state.is_done())
}

pub(in crate::scheme_editor) fn item_is_partial(item: &Item) -> bool {
    item.marker == ItemMarker::Checkbox
        && (item.repeats.is_some() || item.state.iter().any(|state| state.state.is_done()))
        && !item_is_done(item)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::scheme_editor) struct LineChange {
    pub(in crate::scheme_editor) prefix: usize,
    pub(in crate::scheme_editor) old_suffix: usize,
    pub(in crate::scheme_editor) new_suffix: usize,
}

pub(in crate::scheme_editor) fn line_change(old_lines: &[&str], new_lines: &[&str]) -> LineChange {
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

pub(in crate::scheme_editor) fn line_ranges(text: &str) -> Vec<Range<usize>> {
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
