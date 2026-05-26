pub(super) use crate::text_util::{
    byte_offset_to_utf16, byte_range_to_utf16_range, clamp_char_boundary,
    clamp_range_to_char_boundaries, next_char_boundary, next_word_offset, previous_char_boundary,
    previous_word_offset, utf16_range_to_byte_range,
};

pub(super) fn sanitize_input(input: impl Into<String>) -> String {
    input.into().replace(['\r', '\n'], " ")
}
