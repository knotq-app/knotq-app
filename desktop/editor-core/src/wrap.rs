use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrapLine {
    pub text_range: Range<usize>,
    pub is_continuation: bool,
}

pub fn soft_wrap(
    text: &str,
    max_width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<WrapLine> {
    if text.is_empty() {
        return vec![WrapLine {
            text_range: 0..0,
            is_continuation: false,
        }];
    }

    let mut lines = Vec::new();
    let mut start = 0;
    let mut last_break = None;
    for (idx, ch) in text.char_indices() {
        let end = idx + ch.len_utf8();
        if measure(&text[start..end]) <= max_width {
            if ch.is_whitespace() {
                last_break = Some(end);
            }
            continue;
        }
        let break_at = last_break
            .filter(|break_at| *break_at > start)
            .unwrap_or(idx);
        lines.push(WrapLine {
            text_range: start..break_at,
            is_continuation: !lines.is_empty(),
        });
        start = break_at;
        last_break = None;
    }
    lines.push(WrapLine {
        text_range: start..text.len(),
        is_continuation: !lines.is_empty(),
    });
    lines
}
