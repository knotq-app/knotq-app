use std::ops::Range;

/// URL schemes we recognize as the start of a clickable link, plus the bare
/// `www.` host prefix (which is normalized to `https://` when opened).
const PREFIXES: &[&str] = &["https://", "http://", "www."];

/// Characters that end a URL run when encountered (in addition to whitespace
/// and control characters). These rarely appear inside a real URL and reading
/// them as part of one tends to swallow surrounding prose.
const STOP_CHARS: &[char] = &['<', '>', '"', '`', '{', '}', '|', '\\', '^', '\u{fffc}'];

/// Trailing punctuation trimmed from a detected URL so a link at the end of a
/// sentence (e.g. `see https://example.com.`) doesn't capture the period.
const TRAILING_TRIM: &[char] = &['.', ',', ';', ':', '!', '?', '\'', '"', ')', ']', '}', '>'];

/// Byte ranges within `line` that look like URLs, in document order and
/// non-overlapping. Deliberately simple: it matches bare `http(s)://` and
/// `www.` runs rather than markdown `[label](url)` syntax.
pub(super) fn detect_links(line: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut from = 0;
    while from < line.len() {
        let Some(start) = next_candidate(line, from) else {
            break;
        };
        // Require a word boundary before the prefix so we don't light up
        // `xhttps://...` or a scheme embedded mid-token.
        if start == 0 || !preceding_is_url_char(line, start) {
            let end = url_end(line, start);
            if end > start {
                ranges.push(start..end);
                from = end;
                continue;
            }
        }
        from = start + char_len_at(line, start);
    }
    ranges
}

/// The URL to open for a detected `text` span, normalizing a bare `www.` host
/// to an `https://` URL the browser will accept.
pub(super) fn link_url(text: &str) -> String {
    if text.starts_with("www.") {
        format!("https://{text}")
    } else {
        text.to_string()
    }
}

/// Earliest byte index at or after `from` where a known prefix begins.
fn next_candidate(line: &str, from: usize) -> Option<usize> {
    PREFIXES
        .iter()
        .filter_map(|prefix| line[from..].find(prefix).map(|rel| from + rel))
        .min()
}

/// Walks from a prefix `start` to the end of the URL run, then trims trailing
/// punctuation (keeping a closing paren only when it balances an opening one).
fn url_end(line: &str, start: usize) -> usize {
    let mut end = start;
    for (rel, ch) in line[start..].char_indices() {
        if ch.is_whitespace() || ch.is_control() || STOP_CHARS.contains(&ch) {
            break;
        }
        end = start + rel + ch.len_utf8();
    }

    while end > start {
        let span = &line[start..end];
        let last = span.chars().next_back().unwrap();
        // Keep a closing paren that is balanced by an opening one inside the URL
        // (e.g. Wikipedia's `..._(programming_language)`); only trim a stray one.
        if last == ')' && span.matches('(').count() >= span.matches(')').count() {
            break;
        }
        if TRAILING_TRIM.contains(&last) {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    end
}

fn preceding_is_url_char(line: &str, index: usize) -> bool {
    line[..index]
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '/' || ch == '.')
}

fn char_len_at(line: &str, index: usize) -> usize {
    line[index..].chars().next().map(char::len_utf8).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(line: &str) -> Vec<&str> {
        detect_links(line)
            .into_iter()
            .map(|range| &line[range])
            .collect()
    }

    #[test]
    fn detects_bare_https_url() {
        assert_eq!(detected("see https://example.com now"), vec!["https://example.com"]);
    }

    #[test]
    fn trims_trailing_sentence_punctuation() {
        assert_eq!(detected("go to https://example.com."), vec!["https://example.com"]);
        assert_eq!(detected("(https://example.com)"), vec!["https://example.com"]);
    }

    #[test]
    fn keeps_balanced_parens() {
        assert_eq!(
            detected("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            vec!["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
    }

    #[test]
    fn detects_www_and_normalizes_url() {
        let ranges = detect_links("visit www.example.com today");
        assert_eq!(ranges.len(), 1);
        assert_eq!(link_url("www.example.com"), "https://www.example.com");
    }

    #[test]
    fn ignores_scheme_inside_a_word() {
        assert!(detected("xhttps://example.com").is_empty());
    }

    #[test]
    fn detects_multiple_links() {
        assert_eq!(
            detected("a http://a.com b https://b.com"),
            vec!["http://a.com", "https://b.com"]
        );
    }
}
