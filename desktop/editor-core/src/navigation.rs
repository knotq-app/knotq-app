use std::ops::Range;

pub fn previous_char_boundary(text: &str, offset: usize) -> usize {
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

pub fn next_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = (offset + 1).min(text.len());
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

pub fn previous_word_offset(text: &str, offset: usize) -> usize {
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

pub fn next_word_offset(text: &str, offset: usize) -> usize {
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

pub fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let mut offset = clamp_char_boundary(text, offset.min(text.len()));
    if offset == text.len() || current_char(text, offset).is_some_and(|(_, ch)| ch == '\n') {
        offset = previous_char_boundary(text, offset);
    }

    let Some((_, ch)) = current_char(text, offset) else {
        return offset..offset;
    };
    let category = word_category(ch);
    let mut start = offset;
    while let Some((idx, ch)) = previous_char(text, start) {
        if word_category(ch) != category || ch == '\n' {
            break;
        }
        start = idx;
    }

    let mut end = next_char_boundary(text, offset);
    while let Some((_, ch)) = current_char(text, end) {
        if word_category(ch) != category || ch == '\n' {
            break;
        }
        end = next_char_boundary(text, end);
    }

    start..end
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

fn clamp_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
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
