use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct MarkdownStyle {
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) highlight: bool,
    pub(super) strikethrough: bool,
    pub(super) heading: bool,
}

/// Whether a run is visible document content or a markup marker (the `*`, `==`,
/// or leading `#` characters). Markers are rendered on the cursor's line but
/// collapsed away on other lines for an Obsidian-style live preview.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MarkdownRunKind {
    Content,
    Marker,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MarkdownRun {
    pub(super) len: usize,
    pub(super) style: MarkdownStyle,
    pub(super) kind: MarkdownRunKind,
}

#[derive(Clone, Copy)]
enum Emphasis {
    Bold,
    Italic,
    Highlight,
    Strikethrough,
}

impl Emphasis {
    fn apply(self, style: &mut MarkdownStyle) {
        match self {
            Emphasis::Bold => style.bold = true,
            Emphasis::Italic => style.italic = true,
            Emphasis::Highlight => style.highlight = true,
            Emphasis::Strikethrough => style.strikethrough = true,
        }
    }
}

/// Inline emphasis markers, matched longest-first so `**` wins over `*`.
/// Mirrors Obsidian: `**`/`__` bold, `*`/`_` italic, `==` highlight,
/// `~~` strikethrough.
const DELIMITERS: &[(&str, Emphasis)] = &[
    ("**", Emphasis::Bold),
    ("__", Emphasis::Bold),
    ("==", Emphasis::Highlight),
    ("~~", Emphasis::Strikethrough),
    ("*", Emphasis::Italic),
    ("_", Emphasis::Italic),
];

pub(super) fn parse_markdown_runs(line: &str) -> Vec<MarkdownRun> {
    let heading = is_markdown_heading(line);
    let base_style = MarkdownStyle {
        bold: heading,
        italic: false,
        highlight: false,
        strikethrough: false,
        heading,
    };
    let mut runs = Vec::new();
    // The leading `#`/`## ` of a heading is a marker; the rest is parsed as body.
    let body_start = if heading {
        let end = markdown_heading_marker_range(line)
            .map(|range| range.end)
            .unwrap_or(0);
        push_markdown_run(&mut runs, end, base_style, MarkdownRunKind::Marker);
        end
    } else {
        0
    };
    parse_emphasis(&line[body_start..], base_style, &mut runs);
    runs
}

/// Walks `text`, applying any emphasis markers on top of `base_style`. Delimiter
/// characters are emitted as `Marker` runs (styled like the surrounding text);
/// only the wrapped content is restyled, and nested markers parse recursively.
fn parse_emphasis(text: &str, base_style: MarkdownStyle, runs: &mut Vec<MarkdownRun>) {
    let mut index = 0;

    while index < text.len() {
        if let Some((delimiter, emphasis)) = open_delimiter(&text[index..]) {
            let inner_start = index + delimiter.len();
            if let Some(close_rel) = text[inner_start..].find(delimiter) {
                let inner_end = inner_start + close_rel;
                push_markdown_run(runs, delimiter.len(), base_style, MarkdownRunKind::Marker);
                if inner_end > inner_start {
                    let mut inner_style = base_style;
                    emphasis.apply(&mut inner_style);
                    parse_emphasis(&text[inner_start..inner_end], inner_style, runs);
                }
                push_markdown_run(runs, delimiter.len(), base_style, MarkdownRunKind::Marker);
                index = inner_end + delimiter.len();
                continue;
            }
        }

        let ch = text[index..].chars().next().unwrap();
        let ch_len = ch.len_utf8();
        push_markdown_run(runs, ch_len, base_style, MarkdownRunKind::Content);
        index += ch_len;
    }
}

fn open_delimiter(text: &str) -> Option<(&'static str, Emphasis)> {
    DELIMITERS
        .iter()
        .find(|(delimiter, _)| text.starts_with(delimiter))
        .map(|(delimiter, emphasis)| (*delimiter, *emphasis))
}

fn push_markdown_run(
    runs: &mut Vec<MarkdownRun>,
    len: usize,
    style: MarkdownStyle,
    kind: MarkdownRunKind,
) {
    if len == 0 {
        return;
    }
    if let Some(last) = runs.last_mut() {
        // Only merge runs that share styling *and* kind, so markers stay
        // distinguishable from identically-styled adjacent content.
        if last.style == style && last.kind == kind {
            last.len += len;
            return;
        }
    }
    runs.push(MarkdownRun { len, style, kind });
}

pub(super) fn is_markdown_heading(line: &str) -> bool {
    markdown_heading_level(line).is_some()
}

pub(super) fn markdown_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 {
        return None;
    }
    trimmed
        .chars()
        .nth(hashes)
        .is_none_or(char::is_whitespace)
        .then_some(hashes)
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
