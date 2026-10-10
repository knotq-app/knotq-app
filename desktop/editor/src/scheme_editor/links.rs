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

/// What a `[label](target)` target may start with. A link is opened by
/// handing its target to the system, so only web and mail addresses qualify;
/// anything else (`file:`, an app's own scheme) stays plain text.
const NAMED_TARGET_PREFIXES: &[&str] = &["https://", "http://", "www.", "mailto:"];

/// A clickable span of a line and the address it opens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DetectedLink {
    /// The bytes that read as the link: the whole URL for a bare one, the label
    /// for `[label](url)`.
    pub(super) range: Range<usize>,
    pub(super) url: String,
}

/// A `[label](target)` link starting at the beginning of `text`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct NamedLink {
    /// The label, between the brackets.
    pub(super) label: Range<usize>,
    /// The target, between the parentheses.
    pub(super) target: Range<usize>,
    /// Bytes from the `[` through the closing `)`.
    pub(super) len: usize,
}

/// Parses a `[label](target)` link at the start of `text`. The label must be
/// non-empty and hold no brackets, and the target must be a web or mail
/// address with no whitespace — so ordinary bracketed prose (`[x] (later)`,
/// `[1](2)`) is left alone.
pub(super) fn named_link_at(text: &str) -> Option<NamedLink> {
    let rest = text.strip_prefix('[')?;
    let label_len = rest.find([']', '[', '\u{fffc}'])?;
    if label_len == 0 || !rest[label_len..].starts_with("](") {
        return None;
    }
    let target_start = 1 + label_len + 2;
    let target_len = text[target_start..].find(|ch: char| ch == ')' || ch.is_whitespace())?;
    let target = &text[target_start..target_start + target_len];
    if !text[target_start + target_len..].starts_with(')')
        || !NAMED_TARGET_PREFIXES
            .iter()
            .any(|prefix| target.len() > prefix.len() && target.starts_with(prefix))
    {
        return None;
    }
    Some(NamedLink {
        label: 1..1 + label_len,
        target: target_start..target_start + target_len,
        len: target_start + target_len + 1,
    })
}

/// The length of the code inside a `` `code` `` span opening at the start of
/// `text`. An empty pair (` `` `) is not a span.
pub(super) fn code_span_at(text: &str) -> Option<usize> {
    let inner = text.strip_prefix('`')?;
    inner.find('`').filter(|len| *len > 0)
}

/// The links on `line`, in document order and non-overlapping: bare
/// `http(s)://` and `www.` runs, and `[label](url)` links, whose clickable
/// span is the label.
pub(super) fn detect_links(line: &str) -> Vec<DetectedLink> {
    let mut links = Vec::new();
    let mut from = 0;
    while from < line.len() {
        let bare = next_candidate(line, from);
        let named = line[from..].find('[').map(|rel| from + rel);
        let code = line[from..].find('`').map(|rel| from + rel);
        let Some(start) = [bare, named, code].into_iter().flatten().min() else {
            break;
        };
        // Nothing inside a code span is a link.
        if code == Some(start) {
            from = start + code_span_at(&line[start..]).map_or(1, |inner| inner + 2);
            continue;
        }
        if let Some(link) = named_link_at(&line[start..]) {
            links.push(DetectedLink {
                range: start + link.label.start..start + link.label.end,
                url: link_url(&line[start + link.target.start..start + link.target.end]),
            });
            from = start + link.len;
            continue;
        }
        // Require a word boundary before the prefix so we don't light up
        // `xhttps://...` or a scheme embedded mid-token.
        if bare == Some(start) && (start == 0 || !preceding_is_url_char(line, start)) {
            let end = url_end(line, start);
            if end > start {
                links.push(DetectedLink {
                    range: start..end,
                    url: link_url(&line[start..end]),
                });
                from = end;
                continue;
            }
        }
        from = start + char_len_at(line, start);
    }
    links
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
    line[index..]
        .chars()
        .next()
        .map(char::len_utf8)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(line: &str) -> Vec<&str> {
        detect_links(line)
            .into_iter()
            .map(|link| &line[link.range])
            .collect()
    }

    #[test]
    fn detects_bare_https_url() {
        assert_eq!(
            detected("see https://example.com now"),
            vec!["https://example.com"]
        );
    }

    #[test]
    fn trims_trailing_sentence_punctuation() {
        assert_eq!(
            detected("go to https://example.com."),
            vec!["https://example.com"]
        );
        assert_eq!(
            detected("(https://example.com)"),
            vec!["https://example.com"]
        );
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
        let links = detect_links("visit www.example.com today");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://www.example.com");
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

    #[test]
    fn a_named_link_is_clickable_on_its_label_and_opens_its_target() {
        let line = "read [the docs](https://example.com/docs) first";
        let links = detect_links(line);
        assert_eq!(links.len(), 1);
        assert_eq!(&line[links[0].range.clone()], "the docs");
        assert_eq!(links[0].url, "https://example.com/docs");
    }

    #[test]
    fn a_named_link_does_not_also_count_its_target_as_a_bare_url() {
        let line = "[a](https://a.com) then https://b.com and [c](www.c.com)";
        let links = detect_links(line);
        let found: Vec<(&str, &str)> = links
            .iter()
            .map(|link| (&line[link.range.clone()], link.url.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                ("a", "https://a.com"),
                ("https://b.com", "https://b.com"),
                ("c", "https://www.c.com"),
            ]
        );
    }

    #[test]
    fn brackets_that_are_not_a_web_link_stay_text() {
        for line in [
            "[x] (later)",
            "[1](2)",
            "[](https://example.com)",
            "[open](file:///etc/passwd)",
            "[run](javascript:alert(1))",
            "[a](https://)",
            "[a](https://exa mple.com)",
            "[unclosed](https://example.com",
        ] {
            assert!(
                detect_links(line)
                    .iter()
                    .all(|link| line[link.range.clone()].starts_with("http")),
                "{line:?} produced a named link"
            );
            assert_eq!(named_link_at(line), None, "{line}");
        }
    }

    #[test]
    fn nothing_inside_a_code_span_is_a_link() {
        assert!(detected("`curl https://example.com` and `[a](https://a.com)`").is_empty());
        assert_eq!(
            detected("`code` then https://example.com"),
            vec!["https://example.com"]
        );
        assert_eq!(
            detected("a stray ` then https://example.com"),
            vec!["https://example.com"]
        );
    }

    #[test]
    fn a_bare_url_inside_square_brackets_is_still_a_link() {
        assert_eq!(
            detected("[https://example.com]"),
            vec!["https://example.com"]
        );
    }
}
