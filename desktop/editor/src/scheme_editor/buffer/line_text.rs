use std::borrow::Cow;
use std::ops::Range;

use knotq_model::{Item, ItemMarker};

use super::{TABLE_OBJECT_CHAR, TABLE_OBJECT_LEN};

/// Trim leading spaces/tabs and turn any remaining tab into a space.
///
/// Runs on every line on every keystroke (via [`clean_line_text`]), so it
/// borrows the input when there is nothing to strip — the overwhelmingly
/// common case — and only allocates when the line actually needs cleaning.
pub(in crate::scheme_editor) fn clean_display_line_text(text: &str) -> Cow<'_, str> {
    let trimmed = text.trim_start_matches([' ', '\t']);
    if trimmed.contains('\t') {
        Cow::Owned(trimmed.replace('\t', " "))
    } else {
        Cow::Borrowed(trimmed)
    }
}

/// Strip the line's table/image sentinel object char(s), if any. Borrows the
/// input when the line has none, which is nearly always.
pub(in crate::scheme_editor) fn line_without_table_object(line: &str) -> Cow<'_, str> {
    if line.contains(TABLE_OBJECT_CHAR) {
        Cow::Owned(line.replace(TABLE_OBJECT_CHAR, ""))
    } else {
        Cow::Borrowed(line)
    }
}

/// [`clean_display_line_text`] then [`line_without_table_object`], composed
/// without an intermediate allocation when neither step needs to change
/// anything (plain text with no leading whitespace and no block object,
/// which is most lines most of the time).
pub(in crate::scheme_editor) fn clean_line_text(text: &str) -> Cow<'_, str> {
    match clean_display_line_text(text) {
        Cow::Borrowed(displayed) => line_without_table_object(displayed),
        Cow::Owned(displayed) => match line_without_table_object(&displayed) {
            Cow::Borrowed(_) => Cow::Owned(displayed),
            Cow::Owned(stripped) => Cow::Owned(stripped),
        },
    }
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
