use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct MarkdownStyle {
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) heading: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MarkdownRun {
    pub(super) len: usize,
    pub(super) style: MarkdownStyle,
}

pub(super) fn parse_markdown_runs(line: &str) -> Vec<MarkdownRun> {
    let heading = is_markdown_heading(line);
    let base_style = MarkdownStyle {
        bold: heading,
        italic: false,
        heading,
    };
    let mut runs = Vec::new();
    let mut index = 0;

    while index < line.len() {
        let ch = line[index..].chars().next().unwrap();
        let ch_len = ch.len_utf8();

        if matches!(ch, '*' | '_') {
            if let Some(close_rel) = line[index + ch_len..].find(ch) {
                let close = index + ch_len + close_rel;
                push_markdown_run(&mut runs, ch_len, base_style);
                if close > index + ch_len {
                    let mut emphasis = base_style;
                    if ch == '*' {
                        emphasis.bold = true;
                    } else {
                        emphasis.italic = true;
                    }
                    push_markdown_run(&mut runs, close - (index + ch_len), emphasis);
                }
                push_markdown_run(&mut runs, ch_len, base_style);
                index = close + ch_len;
                continue;
            }
        }

        push_markdown_run(&mut runs, ch_len, base_style);
        index += ch_len;
    }

    runs
}

fn push_markdown_run(runs: &mut Vec<MarkdownRun>, len: usize, style: MarkdownStyle) {
    if len == 0 {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.style == style {
            last.len += len;
            return;
        }
    }
    runs.push(MarkdownRun { len, style });
}

pub(super) fn is_markdown_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 {
        return false;
    }
    trimmed.chars().nth(hashes).is_none_or(char::is_whitespace)
}

pub(super) fn markdown_heading_marker_range(line: &str) -> Option<Range<usize>> {
    let leading_ws = line.len() - line.trim_start().len();
    let trimmed = &line[leading_ws..];
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 {
        return None;
    }
    let mut end = leading_ws + hashes;
    if let Some(ch) = line[end..].chars().next().filter(|ch| ch.is_whitespace()) {
        end += ch.len_utf8();
    }
    Some(leading_ws..end)
}
