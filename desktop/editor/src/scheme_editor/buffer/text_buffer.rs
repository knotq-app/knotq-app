//! The editor's text, with its line index kept alongside it.
//!
//! The editor addresses text by `(row, col)`, so nearly everything it does —
//! placing the caret, measuring a line, hit-testing a click, painting a row —
//! needs to know where each line starts. That used to be answered by
//! [`line_ranges`], which scans the whole document and allocates a `Vec` of
//! every line's range. Paint asks per row, so drawing a 2,000-line scheme
//! re-scanned ~140KB and allocated a 2,000-entry `Vec` two thousand times: the
//! single largest cost in the app, ~300ms between pressing a key and seeing the
//! character.
//!
//! The index is derived state, and derived state that can go stale is a
//! correctness bug, not just a performance one — a stale index misplaces the
//! caret or panics on a slice boundary. So the text is private and the only way
//! to change it is [`TextBuffer::set`], which recomputes the index in the same
//! breath. `Deref<Target = str>` keeps every read site reading like a `String`.

use std::ops::Range;

use super::line_ranges;

#[derive(Clone, Debug, Default)]
pub(in crate::scheme_editor) struct TextBuffer {
    text: String,
    /// One range per line, in order. Never empty: an empty document is one
    /// empty line, which is what `line_ranges` returns and what the editor's
    /// row/col model assumes.
    ranges: Vec<Range<usize>>,
}

impl TextBuffer {
    pub(in crate::scheme_editor) fn new(text: String) -> Self {
        let ranges = line_ranges(&text);
        Self { text, ranges }
    }

    /// Replace the whole buffer. The index is rebuilt here and nowhere else.
    pub(in crate::scheme_editor) fn set(&mut self, text: String) {
        self.text = text;
        self.reindex();
    }

    /// Rewrite the buffer in place, reusing its allocation. Same guarantee as
    /// [`set`] — the index is rebuilt before the buffer is readable again — but
    /// without handing over a freshly allocated `String`, which for a large
    /// document is most of the cost of rebuilding it.
    pub(in crate::scheme_editor) fn rewrite(&mut self, fill: impl FnOnce(&mut String)) {
        self.text.clear();
        fill(&mut self.text);
        self.reindex();
    }

    fn reindex(&mut self) {
        self.ranges.clear();
        let mut start = 0;
        for (idx, ch) in self.text.char_indices() {
            if ch == '\n' {
                self.ranges.push(start..idx);
                start = idx + ch.len_utf8();
            }
        }
        self.ranges.push(start..self.text.len());
    }

    /// Every line's byte range, in row order. Always at least one entry.
    pub(in crate::scheme_editor) fn line_ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }

    pub(in crate::scheme_editor) fn line_count(&self) -> usize {
        self.ranges.len()
    }

    pub(in crate::scheme_editor) fn line_range(&self, row: usize) -> Option<Range<usize>> {
        self.ranges.get(row).cloned()
    }

    pub(in crate::scheme_editor) fn line_len(&self, row: usize) -> usize {
        self.ranges
            .get(row)
            .map(|range| range.end - range.start)
            .unwrap_or(0)
    }
}

impl std::ops::Deref for TextBuffer {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl PartialEq<str> for TextBuffer {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index must be exactly what a fresh scan would produce, for every
    /// shape of document the editor can hold — a stale or subtly different
    /// index misplaces the caret rather than merely being slow.
    #[test]
    fn the_index_always_matches_a_fresh_scan() {
        let documents = [
            "",
            "a",
            "\n",
            "a\n",
            "\na",
            "one\ntwo\nthree",
            "trailing\n\n",
            // Multi-byte: ranges are BYTE offsets, so a naive char count drifts.
            "héllo\nwörld\n🙂",
        ];
        let mut buffer = TextBuffer::default();
        for document in documents {
            buffer.set(document.to_string());
            assert_eq!(buffer.line_ranges(), line_ranges(document), "{document:?}");
            assert_eq!(&buffer[..], document);
            assert_eq!(TextBuffer::new(document.to_string()).ranges, buffer.ranges);
        }
    }

    /// Reuse is what makes `set` cheap; it must not leak the previous document.
    #[test]
    fn setting_a_shorter_document_drops_the_old_lines() {
        let mut buffer = TextBuffer::new("a\nb\nc\nd".to_string());
        assert_eq!(buffer.line_count(), 4);

        buffer.set("x".to_string());

        assert_eq!(buffer.line_count(), 1);
        assert_eq!(buffer.line_ranges(), line_ranges("x"));
    }

    #[test]
    fn line_len_and_range_agree_with_the_text() {
        let buffer = TextBuffer::new("alpha\n\nbeta".to_string());

        assert_eq!(buffer.line_len(0), 5);
        assert_eq!(buffer.line_len(1), 0);
        assert_eq!(buffer.line_len(2), 4);
        assert_eq!(buffer.line_len(9), 0);
        assert_eq!(buffer.line_range(2), Some(7..11));
        assert_eq!(&buffer[buffer.line_range(2).unwrap()], "beta");
    }
}
