//! Reuse of shaped lines across relayouts.
//!
//! `relayout` runs on every text change and walks the whole document, so
//! anything it does per row is paid on every keystroke, multiplied by the
//! document's length. Two costs used to live there:
//!
//! * **Shaping.** Typing one character re-ran font shaping and wrapping for
//!   every line in the scheme. A row whose [`LineShapeView`] is unchanged is
//!   still correct, so its shaped line is kept instead.
//! * **Moving.** Even once shaping was skipped, relayout still rebuilt the
//!   whole line vector, so every unchanged row's shaped line — ~1.4KB, its
//!   decoration runs stored inline — was memcpy'd twice per keystroke. In a
//!   10,000-line scheme that was 42MB of memcpy per keypress, the largest cost
//!   left. Now an unchanged row is not touched at all: the key stays where it
//!   is, the line stays where it is, and the comparison runs against a borrowed
//!   [`LineShapeView`] that allocates nothing.
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
use knotq_model::ItemId;

/// Everything `relayout` feeds into shaping one row, borrowed from the
/// document rather than copied out of it.
///
/// This is what the comparison runs against. Reusing a row must not cost an
/// allocation, or the saving is spent on deciding to save.
pub(super) struct LineShapeView<'a> {
    pub(super) line: &'a str,
    pub(super) text_width: Pixels,
    pub(super) hidden_prefix_len: usize,
    pub(super) is_done: bool,
    pub(super) reveal: bool,
    pub(super) header_font: bool,
    pub(super) color: Hsla,
    pub(super) annotation: Option<&'a str>,
    pub(super) media_height: Pixels,
    pub(super) line_height: f32,
}

/// The owned form, kept between passes so the next one has something to
/// compare against.
#[derive(Clone, PartialEq)]
pub(super) struct LineShapeKey {
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
}

impl LineShapeKey {
    /// Built by naming every field, so adding a shaping input to `relayout`
    /// without adding it here is a compile error rather than a stale line.
    fn new(view: &LineShapeView<'_>) -> Self {
        let LineShapeView {
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
        } = *view;
        Self {
            line: line.to_string(),
            text_width,
            hidden_prefix_len,
            is_done,
            reveal,
            header_font,
            color,
            annotation: annotation.map(str::to_string),
            media_height,
            line_height,
        }
    }

    /// Destructures the same fields for the same reason `new` does: a field
    /// left out of the comparison is a stale line on screen.
    fn matches(&self, view: &LineShapeView<'_>) -> bool {
        let LineShapeView {
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
        } = *view;
        // Scalars first — comparing the text is the only part that touches
        // another cache line.
        self.text_width == text_width
            && self.hidden_prefix_len == hidden_prefix_len
            && self.is_done == is_done
            && self.reveal == reveal
            && self.header_font == header_font
            && self.color == color
            && self.media_height == media_height
            && self.line_height == line_height
            && self.annotation.as_deref() == annotation
            && self.line == line
    }
}

/// Which rows a relayout must re-associate before it can compare anything: the
/// span `at..at + removed` of the previous pass's rows was replaced by
/// `inserted` new ones, and everything outside that span survived — shifted, but
/// otherwise the same row.
///
/// This is what makes an insertion cheap. Rows are addressed by index, so
/// pressing Enter near the top of a document renumbers every row below it, and
/// an index-aligned cache would call all of them changed and reshape the whole
/// document. Moving the surviving rows to their new indices instead is one
/// memmove.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RowSplice {
    pub(super) at: usize,
    pub(super) removed: usize,
    pub(super) inserted: usize,
}

/// The span of rows that actually changed between two row-identity lists.
///
/// A single contiguous span, found by matching the common prefix and suffix.
/// Every ordinary edit is one: typing changes one row, Enter inserts one,
/// Backspace at a line start removes one. Something that genuinely reorders
/// rows falls back to one span covering the whole reordered region, which
/// reshapes it — correct, just not optimal.
fn row_splice(old: &[ItemId], new: &[ItemId]) -> RowSplice {
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let mut suffix = 0;
    while suffix < max_suffix && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix] {
        suffix += 1;
    }
    RowSplice {
        at: prefix,
        removed: old.len() - prefix - suffix,
        inserted: new.len() - prefix - suffix,
    }
}

/// Per-row shaping keys, held across relayouts.
///
/// Index-aligned with `LineMap`'s rows: `entries[row]` describes the line
/// currently sitting at `row`. The two are spliced together at the start of a
/// pass, so a row that reports "unchanged" is always the row whose shaped line
/// is still in place.
#[derive(Default)]
pub(super) struct ShapeCache {
    entries: Vec<Option<LineShapeKey>>,
    /// The row identities the entries belong to, so an insertion or deletion
    /// can be recognised as a *shift* rather than as every later row changing.
    /// One item is one row, so its id identifies the row across relayouts even
    /// though its index does not.
    ids: Vec<ItemId>,
    /// The document font the recorded keys were shaped under.
    ///
    /// Font size follows the line's heading level, so it is already implied by
    /// the line text — but the FACE comes from the window's text style and is
    /// the same for every row, so it is not in the per-row key. If it changes,
    /// every row must reshape, and nothing in a row's own key would say so.
    font: Option<Font>,
}

impl ShapeCache {
    /// Start a pass over the rows identified by `ids`, returning the splice the
    /// caller must apply to the line map so the two stay aligned.
    ///
    /// A different font discards every key, so every row reshapes.
    pub(super) fn begin(&mut self, ids: &[ItemId], font: &Font) -> RowSplice {
        if self.font.as_ref() != Some(font) {
            self.font = Some(font.clone());
            self.entries.clear();
            self.ids.clear();
        }
        let splice = row_splice(&self.ids, ids);
        self.entries.splice(
            splice.at..splice.at + splice.removed,
            std::iter::repeat_with(|| None).take(splice.inserted),
        );
        self.ids.clear();
        self.ids.extend_from_slice(ids);
        splice
    }

    /// Force the key list to `rows` entries, so it matches the line map even if
    /// the row list and the text buffer ever disagreed about the row count.
    pub(super) fn resize_rows(&mut self, rows: usize) {
        self.entries.resize_with(rows, || None);
    }

    /// Whether `row`'s shaped line is still correct for `view`, in which case
    /// relayout leaves both the key and the line where they are.
    pub(super) fn row_is_unchanged(&self, row: usize, view: &LineShapeView<'_>) -> bool {
        self.entries
            .get(row)
            .and_then(Option::as_ref)
            .is_some_and(|key| key.matches(view))
    }

    /// Record a row that was just reshaped. `None` for a row whose inputs are
    /// not self-contained (a table row), which must always reshape.
    pub(super) fn record_shaped(&mut self, row: usize, view: Option<&LineShapeView<'_>>) {
        if let Some(entry) = self.entries.get_mut(row) {
            *entry = view.map(LineShapeKey::new);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    fn view(line: &str) -> LineShapeView<'_> {
        LineShapeView {
            line,
            text_width: px(400.0),
            hidden_prefix_len: 0,
            is_done: false,
            reveal: false,
            header_font: false,
            color: Hsla::default(),
            annotation: None,
            media_height: px(0.0),
            line_height: 1.0,
        }
    }

    /// Stable ids so a test can talk about "the row that says alpha".
    fn id_for(line: &str) -> ItemId {
        let mut bytes = [0u8; 16];
        for (slot, byte) in bytes.iter_mut().zip(line.bytes()) {
            *slot = byte;
        }
        ItemId(uuid::Uuid::from_bytes(bytes))
    }

    fn ids_for(lines: &[&str]) -> Vec<ItemId> {
        lines.iter().copied().map(id_for).collect()
    }

    fn cache_of(lines: &[&str]) -> ShapeCache {
        let mut cache = ShapeCache::default();
        cache.begin(&ids_for(lines), &gpui::font("Helvetica"));
        for (row, line) in lines.iter().enumerate() {
            cache.record_shaped(row, Some(&view(line)));
        }
        cache
    }

    #[test]
    fn a_row_whose_inputs_are_unchanged_needs_no_reshape() {
        let cache = cache_of(&["alpha", "beta"]);

        assert!(cache.row_is_unchanged(0, &view("alpha")));
        assert!(cache.row_is_unchanged(1, &view("beta")));
    }

    #[test]
    fn changing_the_text_invalidates_only_that_row() {
        let cache = cache_of(&["alpha", "beta"]);

        // Editing row 0 must not make row 1 re-shape: that is the whole point,
        // since a keystroke changes one line and used to re-shape all of them.
        assert!(!cache.row_is_unchanged(0, &view("alpha!")));
        assert!(cache.row_is_unchanged(1, &view("beta")));
    }

    #[test]
    fn every_input_participates_in_the_comparison() {
        let cache = cache_of(&["alpha"]);

        // Each of these is an input shaping actually reads; flipping any one of
        // them must invalidate the row, or the line renders stale.
        let mut width = view("alpha");
        width.text_width = px(401.0);
        let mut done = view("alpha");
        done.is_done = true;
        let mut reveal = view("alpha");
        reveal.reveal = true;
        let mut prefix = view("alpha");
        prefix.hidden_prefix_len = 1;
        let mut header = view("alpha");
        header.header_font = true;
        let mut color = view("alpha");
        color.color = Hsla {
            h: 0.5,
            s: 0.5,
            l: 0.5,
            a: 1.0,
        };
        let mut annotation = view("alpha");
        annotation.annotation = Some("due");
        let mut media = view("alpha");
        media.media_height = px(10.0);
        let mut height = view("alpha");
        height.line_height = 2.0;
        let text = view("alpha!");

        for changed in [
            width, done, reveal, prefix, header, color, annotation, media, height, text,
        ] {
            assert!(
                !cache.row_is_unchanged(0, &changed),
                "a changed shaping input did not invalidate the row"
            );
        }
    }

    /// Changing the document font must discard every cached row: the face is
    /// shared by all rows, so no individual row's key records it.
    #[test]
    fn changing_the_font_discards_every_row() {
        let mut cache = cache_of(&["alpha"]);

        // Same font: the row survives.
        cache.begin(&ids_for(&["alpha"]), &gpui::font("Helvetica"));
        assert!(cache.row_is_unchanged(0, &view("alpha")));

        // Different font: it does not.
        cache.begin(&ids_for(&["alpha"]), &gpui::font("Courier"));
        assert!(!cache.row_is_unchanged(0, &view("alpha")));
    }

    /// Rows added by a longer document must never report as unchanged — there
    /// is no shaped line behind them yet, only a placeholder.
    #[test]
    fn rows_added_by_a_longer_document_always_reshape() {
        let mut cache = cache_of(&["alpha"]);
        cache.begin(&ids_for(&["alpha", "beta", "gamma"]), &gpui::font("Helvetica"));

        assert_eq!(cache.len(), 3);
        assert!(cache.row_is_unchanged(0, &view("alpha")));
        assert!(!cache.row_is_unchanged(1, &view("beta")));
        assert!(!cache.row_is_unchanged(2, &view("gamma")));
    }

    /// And a shorter document must drop the tail, so the keys stay aligned with
    /// the line map's rows.
    #[test]
    fn rows_removed_by_a_shorter_document_are_dropped() {
        let mut cache = cache_of(&["alpha", "beta", "gamma"]);
        cache.begin(&ids_for(&["alpha"]), &gpui::font("Helvetica"));

        assert_eq!(cache.len(), 1);
        assert!(cache.row_is_unchanged(0, &view("alpha")));
    }

    /// THE point of tracking identity: inserting a row near the top must leave
    /// every row below it reusable. Index alignment alone would renumber them
    /// all and re-shape the whole document.
    #[test]
    fn inserting_a_row_at_the_top_leaves_the_rest_reusable() {
        let mut cache = cache_of(&["alpha", "beta", "gamma"]);

        let splice = cache.begin(
            &ids_for(&["new", "alpha", "beta", "gamma"]),
            &gpui::font("Helvetica"),
        );

        assert_eq!(
            splice,
            RowSplice {
                at: 0,
                removed: 0,
                inserted: 1
            }
        );
        assert!(!cache.row_is_unchanged(0, &view("new")));
        assert!(cache.row_is_unchanged(1, &view("alpha")));
        assert!(cache.row_is_unchanged(2, &view("beta")));
        assert!(cache.row_is_unchanged(3, &view("gamma")));
    }

    #[test]
    fn deleting_a_row_at_the_top_leaves_the_rest_reusable() {
        let mut cache = cache_of(&["alpha", "beta", "gamma"]);

        let splice = cache.begin(&ids_for(&["beta", "gamma"]), &gpui::font("Helvetica"));

        assert_eq!(
            splice,
            RowSplice {
                at: 0,
                removed: 1,
                inserted: 0
            }
        );
        assert!(cache.row_is_unchanged(0, &view("beta")));
        assert!(cache.row_is_unchanged(1, &view("gamma")));
    }

    /// A row replaced in the middle invalidates only itself.
    #[test]
    fn replacing_a_row_in_the_middle_invalidates_only_it() {
        let mut cache = cache_of(&["alpha", "beta", "gamma"]);

        let splice = cache.begin(
            &ids_for(&["alpha", "other", "gamma"]),
            &gpui::font("Helvetica"),
        );

        assert_eq!(
            splice,
            RowSplice {
                at: 1,
                removed: 1,
                inserted: 1
            }
        );
        assert!(cache.row_is_unchanged(0, &view("alpha")));
        assert!(!cache.row_is_unchanged(1, &view("other")));
        assert!(cache.row_is_unchanged(2, &view("gamma")));
    }

    /// The splice must describe a change the caller can apply blindly to a
    /// parallel vector and end up with the same rows in the same places. This
    /// is the invariant the line map depends on.
    #[test]
    fn the_splice_transforms_the_old_row_list_into_the_new_one() {
        let cases: [(&[&str], &[&str]); 8] = [
            (&[], &["a"]),
            (&["a"], &[]),
            (&["a", "b", "c"], &["a", "b", "c"]),
            (&["a", "b", "c"], &["x", "a", "b", "c"]),
            (&["a", "b", "c"], &["a", "b", "c", "x"]),
            (&["a", "b", "c"], &["a", "x", "y", "c"]),
            (&["a", "b", "c"], &["c", "b", "a"]),
            (&["a", "a2", "b"], &["a", "b"]),
        ];
        for (old, new) in cases {
            let old_ids = ids_for(old);
            let new_ids = ids_for(new);
            let splice = row_splice(&old_ids, &new_ids);

            let mut applied = old_ids.clone();
            applied.splice(
                splice.at..splice.at + splice.removed,
                new_ids[splice.at..splice.at + splice.inserted].iter().copied(),
            );

            assert_eq!(applied, new_ids, "{old:?} -> {new:?} via {splice:?}");
        }
    }

    #[test]
    fn a_row_with_no_recorded_key_is_never_reused() {
        // A table row records `None`: its width depends on other rows, so its
        // key is not self-contained and it must always re-shape.
        let mut cache = ShapeCache::default();
        cache.begin(&ids_for(&["alpha", "beta"]), &gpui::font("Helvetica"));
        cache.record_shaped(0, None);
        cache.record_shaped(1, Some(&view("beta")));

        assert!(!cache.row_is_unchanged(0, &view("alpha")));
        assert!(cache.row_is_unchanged(1, &view("beta")));
    }
}
