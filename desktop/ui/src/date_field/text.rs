use std::ops::Range;

pub(super) fn clamp_range(range: Range<usize>, len: usize) -> Range<usize> {
    let start = range.start.min(len);
    let end = range.end.min(len).max(start);
    start..end
}

pub fn sanitize_numeric_component(raw: &str, max_len: usize) -> String {
    raw.chars()
        .filter(|ch| ch.is_ascii_digit())
        .take(max_len)
        .collect()
}
