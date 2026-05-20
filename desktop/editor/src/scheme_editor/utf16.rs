use std::ops::Range;

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
    let mut byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    text[..byte].encode_utf16().count()
}
