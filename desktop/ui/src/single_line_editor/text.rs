use std::ops::Range;

pub(super) fn sanitize_input(input: impl Into<String>) -> String {
    input.into().replace(['\r', '\n'], " ")
}

pub(super) fn previous_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    if offset == 0 {
        return 0;
    }
    offset -= 1;
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub(super) fn next_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = (offset + 1).min(text.len());
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

pub(super) fn previous_word_offset(text: &str, offset: usize) -> usize {
    let mut cursor = clamp_char_boundary(text, offset);
    while let Some((idx, ch)) = previous_char(text, cursor) {
        if !ch.is_whitespace() {
            break;
        }
        cursor = idx;
    }

    let Some((idx, ch)) = previous_char(text, cursor) else {
        return 0;
    };
    let category = word_category(ch);
    cursor = idx;
    while let Some((idx, ch)) = previous_char(text, cursor) {
        if ch.is_whitespace() || word_category(ch) != category {
            break;
        }
        cursor = idx;
    }
    cursor
}

pub(super) fn next_word_offset(text: &str, offset: usize) -> usize {
    let mut cursor = clamp_char_boundary(text, offset);
    while let Some((_, ch)) = current_char(text, cursor) {
        if !ch.is_whitespace() {
            break;
        }
        cursor = next_char_boundary(text, cursor);
    }

    let Some((_, ch)) = current_char(text, cursor) else {
        return text.len();
    };
    let category = word_category(ch);
    while let Some((_, ch)) = current_char(text, cursor) {
        if ch.is_whitespace() || word_category(ch) != category {
            break;
        }
        cursor = next_char_boundary(text, cursor);
    }
    cursor
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WordCategory {
    Word,
    Whitespace,
    Punctuation,
}

fn word_category(ch: char) -> WordCategory {
    if ch.is_alphanumeric() || ch == '_' {
        WordCategory::Word
    } else if ch.is_whitespace() {
        WordCategory::Whitespace
    } else {
        WordCategory::Punctuation
    }
}

fn previous_char(text: &str, offset: usize) -> Option<(usize, char)> {
    let offset = clamp_char_boundary(text, offset);
    text[..offset].char_indices().next_back()
}

fn current_char(text: &str, offset: usize) -> Option<(usize, char)> {
    let offset = clamp_char_boundary(text, offset);
    text.get(offset..)?
        .char_indices()
        .next()
        .map(|(i, ch)| (offset + i, ch))
}

pub(super) fn clamp_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub(super) fn clamp_range_to_char_boundaries(text: &str, range: Range<usize>) -> Range<usize> {
    let start = clamp_char_boundary(text, range.start);
    let end = clamp_char_boundary(text, range.end).max(start);
    start..end
}

pub(super) fn utf16_range_to_byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    utf16_offset_to_byte(text, range.start)..utf16_offset_to_byte(text, range.end)
}

fn utf16_offset_to_byte(text: &str, target: usize) -> usize {
    if target == 0 {
        return 0;
    }
    let mut units = 0;
    for (idx, ch) in text.char_indices() {
        if units >= target {
            return idx;
        }
        units += ch.len_utf16();
        if units > target {
            return idx;
        }
    }
    text.len()
}

pub(super) fn byte_range_to_utf16_range(text: &str, range: Range<usize>) -> Range<usize> {
    byte_offset_to_utf16(text, range.start)..byte_offset_to_utf16(text, range.end)
}

pub(super) fn byte_offset_to_utf16(text: &str, byte: usize) -> usize {
    let byte = clamp_char_boundary(text, byte);
    text[..byte].encode_utf16().count()
}
