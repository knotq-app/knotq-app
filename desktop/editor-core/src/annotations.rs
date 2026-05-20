use std::ops::Range;

use chrono::{DateTime, Local, Utc};
use knotq_date_util::format_time;
use knotq_model::{Item, TimeFormat};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MarkdownStyle {
    pub bold: bool,
    pub italic: bool,
    pub heading: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkdownRun {
    pub len: usize,
    pub style: MarkdownStyle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Annotation {
    Bold(Range<usize>),
    Italic(Range<usize>),
    Date { text: String },
    Repeat { text: String },
}

pub fn compute_annotations(item: &Item, time_format: TimeFormat) -> Vec<Annotation> {
    let mut annotations = markdown_annotations(&item.text);
    if let Some(text) = date_annotation_text(item, time_format) {
        annotations.push(Annotation::Date { text });
    }
    if item.repeats.is_some() {
        annotations.push(Annotation::Repeat {
            text: "Repeats".to_string(),
        });
    }
    annotations
}

pub fn parse_markdown_runs(line: &str) -> Vec<MarkdownRun> {
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

pub fn is_markdown_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    hashes > 0 && trimmed.chars().nth(hashes).is_none_or(char::is_whitespace)
}

pub fn markdown_heading_marker_range(line: &str) -> Option<Range<usize>> {
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

fn markdown_annotations(text: &str) -> Vec<Annotation> {
    let mut annotations = Vec::new();
    let mut offset = 0;
    for run in parse_markdown_runs(text) {
        let range = offset..offset + run.len;
        if run.style.bold {
            annotations.push(Annotation::Bold(range.clone()));
        }
        if run.style.italic {
            annotations.push(Annotation::Italic(range));
        }
        offset += run.len;
    }
    annotations
}

fn date_annotation_text(item: &Item, time_format: TimeFormat) -> Option<String> {
    match (item.start, item.end) {
        (Some(start), Some(end)) => Some(format!(
            "{} -> {}",
            date_label(start, time_format),
            date_label(end, time_format)
        )),
        (Some(start), None) => Some(format!("Start {}", date_label(start, time_format))),
        (None, Some(end)) => Some(format!("Due {}", date_label(end, time_format))),
        _ => None,
    }
}

fn date_label(dt: DateTime<Utc>, time_format: TimeFormat) -> String {
    let local = dt.with_timezone(&Local);
    format!(
        "{} {}",
        local.format("%b %-d"),
        format_time(time_format, local)
    )
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
