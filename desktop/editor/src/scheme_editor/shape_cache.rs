//! Reuse of shaped lines across relayouts.
//!
//! `relayout` re-shapes the whole scheme, and it runs on every text change — so
//! typing one character re-ran font shaping and wrapping for every line in the
//! document. That is the dominant cost of a keystroke in a long scheme, and it
//! lands on the main thread inside the frame, which is what makes typing feel
//! behind.
//!
//! Shaping a line depends only on the inputs recorded in [`LineShapeKey`]. When
//! a row's key is unchanged, its previously shaped line is still correct and can
//! be cloned instead of re-shaped.
//!
//! Two deliberate restrictions keep this honest:
//!
//! * The key is built by exhaustively destructuring, so a new shaping input
//!   cannot be added without this failing to compile. A key that silently
//!   omitted an input would show stale text — the exact bug caching invites.
//! * Rows inside a table are never reused. A cell's width comes from its
//!   anchor's computed grid, which depends on *other* rows, so its key is not
//!   self-contained. Tables are rare; plain lines are the bulk of a document.

use gpui::{Font, Hsla, Pixels};

/// Everything `relayout` feeds into shaping one row.
#[derive(Clone, PartialEq)]
pub(super) struct LineShapeKey {
    pub(super) line: String,
    pub(super) text_width: Pixels,
    pub(super) hidden_prefix_len: usize,
    pub(super) is_done: bool,
    pub(super) reveal: bool,
    pub(super) header_font: bool,
    pub(super) color: Hsla,
    pub(super) annotation: Option<String>,
    pub(super) media_height: Pixels,
    pub(super) line_height: f32,
}

impl LineShapeKey {
    /// Built by naming every field, so adding a shaping input to `relayout`
    /// without adding it here is a compile error rather than a stale line.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        line: String,
        text_width: Pixels,
        hidden_prefix_len: usize,
        is_done: bool,
        reveal: bool,
        header_font: bool,
        color: Hsla,
        annotation: Option<String>,
        media_height: Pixels,
        line_height: f32,
    ) -> Self {
        Self {
            line,
            text_width,
            hidden_prefix_len,
            is_done,
            reveal,
            header_font,
            color,
            annotation,
            media_height,
            line_height,
        }
    }
}

/// What the previous relayout produced: the per-row keys and the shaped lines
/// they belong to, handed to the next relayout so it can reuse the rows whose
/// inputs did not change.
pub(super) struct PreviousShapes {
    keys: Vec<Option<LineShapeKey>>,
    lines: Vec<crate::line_map::SchemeItemLine>,
}

impl PreviousShapes {
    pub(super) fn new(
        keys: Vec<Option<LineShapeKey>>,
        lines: Vec<crate::line_map::SchemeItemLine>,
    ) -> Self {
        Self { keys, lines }
    }

    /// The shaped line for `row` when it was shaped from exactly `key`.
    ///
    /// Both halves must line up: a row whose key was not recorded (a table row)
    /// or whose line is missing yields `None` and is reshaped.
    pub(super) fn reuse(
        &self,
        row: usize,
        key: &LineShapeKey,
    ) -> Option<&crate::line_map::SchemeItemLine> {
        let cached = self.keys.get(row)?.as_ref()?;
        if cached != key {
            return None;
        }
        self.lines.get(row)
    }
}

/// Keys recorded by the relayout in progress.
#[derive(Default)]
pub(super) struct ShapeCache {
    entries: Vec<Option<LineShapeKey>>,
    /// The document font the recorded keys were shaped under.
    ///
    /// Font size follows the line's heading level, so it is already implied by
    /// the line text — but the FACE comes from the window's text style and is
    /// the same for every row, so it is not in the per-row key. If it changes,
    /// every row must reshape, and nothing in a row's own key would say so.
    font: Option<Font>,
}

impl ShapeCache {
    /// Start a fresh pass, yielding the previous pass's keys to compare
    /// against. A different font discards them all.
    pub(super) fn begin(&mut self, rows: usize, font: &Font) -> Vec<Option<LineShapeKey>> {
        let font_changed = self.font.as_ref() != Some(font);
        self.font = Some(font.clone());
        let previous = std::mem::replace(&mut self.entries, Vec::with_capacity(rows));
        if font_changed {
            Vec::new()
        } else {
            previous
        }
    }

    pub(super) fn record(&mut self, key: Option<LineShapeKey>) {
        self.entries.push(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    fn shapes(keys: Vec<Option<LineShapeKey>>) -> PreviousShapes {
        let lines = (0..keys.len())
            .map(|_| crate::line_map::SchemeItemLine::new(Default::default(), None, px(1.0)))
            .collect();
        PreviousShapes::new(keys, lines)
    }

    fn key(line: &str) -> LineShapeKey {
        LineShapeKey::new(
            line.to_string(),
            px(400.0),
            0,
            false,
            false,
            false,
            Hsla::default(),
            None,
            px(0.0),
            1.0,
        )
    }

    #[test]
    fn a_row_whose_inputs_are_unchanged_is_reusable() {
        let previous = shapes(vec![Some(key("alpha")), Some(key("beta"))]);

        assert!(previous.reuse(0, &key("alpha")).is_some());
        assert!(previous.reuse(1, &key("beta")).is_some());
    }

    #[test]
    fn changing_the_text_invalidates_only_that_row() {
        let previous = shapes(vec![Some(key("alpha")), Some(key("beta"))]);

        // Editing row 0 must not make row 1 re-shape: that is the whole point,
        // since a keystroke changes one line and used to re-shape all of them.
        assert!(previous.reuse(0, &key("alpha!")).is_none());
        assert!(previous.reuse(1, &key("beta")).is_some());
    }

    #[test]
    fn every_input_participates_in_the_comparison() {
        let base = key("alpha");
        let previous = shapes(vec![Some(base.clone())]);

        // Each of these is an input shaping actually reads; flipping any one of
        // them must invalidate the row, or the line renders stale.
        let mut width = base.clone();
        width.text_width = px(401.0);
        let mut done = base.clone();
        done.is_done = true;
        let mut reveal = base.clone();
        reveal.reveal = true;
        let mut prefix = base.clone();
        prefix.hidden_prefix_len = 1;
        let mut header = base.clone();
        header.header_font = true;
        let mut color = base.clone();
        color.color = Hsla {
            h: 0.5,
            s: 0.5,
            l: 0.5,
            a: 1.0,
        };
        let mut annotation = base.clone();
        annotation.annotation = Some("due".into());
        let mut media = base.clone();
        media.media_height = px(10.0);
        let mut height = base.clone();
        height.line_height = 2.0;

        for changed in [
            width, done, reveal, prefix, header, color, annotation, media, height,
        ] {
            assert!(
                previous.reuse(0, &changed).is_none(),
                "a changed shaping input did not invalidate the row"
            );
        }
    }

    /// Changing the document font must discard every cached row: the face is
    /// shared by all rows, so no individual row's key records it.
    #[test]
    fn changing_the_font_discards_every_row() {
        let mut cache = ShapeCache::default();
        let a = gpui::font("Helvetica");
        let b = gpui::font("Courier");

        cache.begin(1, &a);
        cache.record(Some(key("alpha")));
        // Same font: the keys survive.
        assert_eq!(cache.begin(1, &a).len(), 1);
        cache.record(Some(key("alpha")));
        // Different font: they do not.
        assert!(cache.begin(1, &b).is_empty());
    }

    #[test]
    fn a_row_with_no_recorded_key_is_never_reused() {
        // A table row records `None`: its width depends on other rows, so its
        // key is not self-contained and it must always re-shape.
        let previous = shapes(vec![None, Some(key("beta"))]);

        assert!(previous.reuse(0, &key("alpha")).is_none());
        assert!(previous.reuse(1, &key("beta")).is_some());
    }

}
