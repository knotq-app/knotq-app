use std::ops::Range;

// ---------------------------------------------------------------------------
// UTF-16 / byte-offset conversions
// ---------------------------------------------------------------------------

pub fn utf16_range_to_byte_range(text: &str, range: Range<usize>) -> Range<usize> {
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

pub fn byte_range_to_utf16_range(text: &str, range: Range<usize>) -> Range<usize> {
    byte_offset_to_utf16(text, range.start)..byte_offset_to_utf16(text, range.end)
}

pub fn byte_offset_to_utf16(text: &str, byte: usize) -> usize {
    let byte = clamp_char_boundary(text, byte);
    text[..byte].encode_utf16().count()
}

// ---------------------------------------------------------------------------
// Char-boundary range helper
// ---------------------------------------------------------------------------

pub fn clamp_range_to_char_boundaries(text: &str, range: Range<usize>) -> Range<usize> {
    let start = clamp_char_boundary(text, range.start);
    let end = clamp_char_boundary(text, range.end).max(start);
    start..end
}

// ---------------------------------------------------------------------------
// Flat (single-line) text selection
// ---------------------------------------------------------------------------

/// A two-point (anchor + head) selection used by single-line text fields.
///
/// Both [`single_line_editor`] and [`date_field`] re-export this type under
/// their own local names (`TextSelection` / `DateFieldSelection`) so that the
/// internal code in each sub-module is unchanged.
#[derive(Clone, Copy, Debug)]
pub struct FlatSelection {
    pub anchor: usize,
    pub head: usize,
}

impl FlatSelection {
    pub fn collapsed(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub fn ordered(self) -> (usize, usize) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub fn reversed(self) -> bool {
        self.head < self.anchor
    }
}

// ---------------------------------------------------------------------------
// Word categories / navigation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordCategory {
    Word,
    Whitespace,
    Punctuation,
}

pub fn word_category(ch: char) -> WordCategory {
    if ch.is_alphanumeric() || ch == '_' {
        WordCategory::Word
    } else if ch.is_whitespace() {
        WordCategory::Whitespace
    } else {
        WordCategory::Punctuation
    }
}

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

pub fn clamp_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
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

pub fn previous_char(text: &str, offset: usize) -> Option<(usize, char)> {
    let offset = clamp_char_boundary(text, offset);
    text[..offset].char_indices().next_back()
}

pub fn current_char(text: &str, offset: usize) -> Option<(usize, char)> {
    let offset = clamp_char_boundary(text, offset);
    text.get(offset..)?
        .char_indices()
        .next()
        .map(|(i, ch)| (offset + i, ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_navigation_matches_editor_boundaries() {
        let text = "alpha beta.gamma";
        assert_eq!(previous_word_offset(text, "alpha beta".len()), 6);
        assert_eq!(next_word_offset(text, 0), 5);
        assert_eq!(next_word_offset(text, 6), 10);
        assert_eq!(word_range_at(text, 7), 6..10);
        assert_eq!(word_range_at(text, 10), 10..11);
    }

    #[test]
    fn word_range_at_line_end_selects_previous_word_not_newline() {
        let text = "# ICPC\nJhala Office Hours";
        assert_eq!(word_range_at(text, "# ICPC".len()), 2.."# ICPC".len());
    }
}
