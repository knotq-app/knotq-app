//! The glyph-family picker, opened by holding one of the bullet/number toolbar
//! buttons.
//!
//! Families change appearance only — a numbered line stays ordered whichever
//! numerals it shows — so this is deliberately an override on a button the user
//! already knows rather than a new top-level control.

use gpui::prelude::*;
use gpui::{deferred, div, px, ClickEvent, Context, IntoElement};
use knotq_model::{ItemMarker, MarkerFamily, MarkerGlyph};
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

const PICKER_WIDTH: f32 = 188.0;
const ROW_HEIGHT: f32 = 28.0;

/// What a family is called in the picker. `Inherit` is described by what it
/// does — following the indent — rather than by a glyph name, because that is
/// the choice the user is actually making.
fn family_label(family: MarkerFamily) -> &'static str {
    match family {
        MarkerFamily::Standard => "Standard",
        MarkerFamily::Discs => "Discs",
        MarkerFamily::Rings => "Rings",
        MarkerFamily::Squares => "Squares",
        MarkerFamily::Dashes => "Dashes",
        MarkerFamily::Alternating => "Alternating",
        MarkerFamily::Decimal => "Numbers",
        MarkerFamily::Alpha => "Letters",
        MarkerFamily::Roman => "Roman",
        MarkerFamily::Outline => "Outline",
    }
}

fn glyph_text(glyph: MarkerGlyph, ordinal: usize) -> String {
    match glyph {
        MarkerGlyph::Disc => "\u{25cf}".into(),
        MarkerGlyph::Circle => "\u{25cb}".into(),
        MarkerGlyph::Square => "\u{25aa}".into(),
        MarkerGlyph::Dash => "\u{2013}".into(),
        numbered => format!("{}.", numbered.ordinal_label(ordinal)),
    }
}

/// A short preview of the glyph, so the list reads as what it will look like
/// rather than as a list of words.
/// Preview the first three depths, so the row shows what the family DOES —
/// a sequence — rather than just its glyph at depth 0. Families that repeat one
/// glyph collapse to that glyph rather than repeating it three times.
fn family_preview(family: MarkerFamily, marker: ItemMarker) -> String {
    // The SAME ordinal at every depth. What varies between depths in a preview
    // must be the glyph style the family chooses, not the count — advancing the
    // ordinal too showed decimal as "1. 2. 3.", which reads as three levels of
    // a nested list when it is one style repeated, and hid that the family is
    // uniform at all.
    let glyphs: Vec<String> = (0..3u8)
        .map(|depth| glyph_text(family.glyph_at(marker, depth), 1))
        .collect();
    if glyphs.iter().all(|g| *g == glyphs[0]) {
        glyphs[0].clone()
    } else {
        glyphs.join(" ")
    }
}

impl KnotQApp {
    pub(crate) fn render_marker_family_picker(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let (marker, anchor) = self.marker_family_picker?;
        let t = self.theme();
        let viewport = window.viewport_size();
        let choices = MarkerFamily::choices_for(marker);
        if choices.is_empty() {
            return None;
        }
        let active = self
            .scheme_editor
            .as_ref()
            .map(|(_, editor)| editor.read(cx).selection_marker_family())
            .unwrap_or(MarkerFamily::Standard);

        let height = px(ROW_HEIGHT * choices.len() as f32 + 8.0);
        let left = clamped_popover_left(
            anchor.x - px(PICKER_WIDTH / 2.0),
            px(PICKER_WIDTH),
            px(f32::from(viewport.width)),
        );
        // Anchored ABOVE the press: the marker buttons live in the toolbar at
        // the bottom of the editor, so opening downward would run off-screen.
        let top = popover_top_biased_below(
            anchor.y - height - px(10.0),
            height,
            px(f32::from(viewport.height)),
        );

        let rows = choices
            .iter()
            .copied()
            .map(|family| self.marker_family_row(family, marker, family == active, t, cx))
            .collect::<Vec<_>>();

        let scrim = div()
            .id("marker-family-scrim")
            .absolute()
            .inset_0()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.marker_family_picker = None;
                cx.notify();
            }));

        let card = div()
            .id("marker-family-picker")
            .absolute()
            .left(left)
            .top(top)
            .w(px(PICKER_WIDTH))
            .p(px(4.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(token_rgba(t.border_main))
            .bg(token_rgba(t.bg_modal))
            .shadow_md()
            .flex()
            .flex_col()
            .gap(px(1.0))
            // Keep the press that picks a family off whatever is behind it.
            .occlude()
            .children(rows);

        Some(deferred(div().absolute().inset_0().child(scrim).child(card)).into_any_element())
    }

    fn marker_family_row(
        &self,
        family: MarkerFamily,
        marker: ItemMarker,
        active: bool,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let editor = self.scheme_editor.as_ref().map(|(_, editor)| editor.clone());
        div()
            .id(("marker-family-row", family as usize))
            .h(px(ROW_HEIGHT))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .when(active, |s| s.bg(token_rgba(t.row_hover)))
            .when(!active, {
                let hover = t.row_hover;
                move |s| s.hover(move |h| h.bg(token_rgba(hover)))
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                if let Some(editor) = editor.clone() {
                    editor.update(cx, |editor, cx| {
                        editor.set_marker_family_for_selection(family, cx)
                    });
                }
                this.marker_family_picker = None;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(20.0))
                    .text_size(px(12.0))
                    .text_color(token_hsla(t.text_dim))
                    .child(family_preview(family, marker)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(token_hsla(t.text_primary))
                    .child(family_label(family)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A family is a glyph SEQUENCE indexed by depth, so the preview has to
    /// show the sequence — one glyph would misrepresent every family whose
    /// whole point is that it changes as the list nests.
    #[test]
    fn previews_show_the_first_three_depths_of_the_sequence() {
        assert_eq!(
            family_preview(MarkerFamily::Standard, ItemMarker::Bullet),
            "● ○ ▪"
        );
        assert_eq!(
            family_preview(MarkerFamily::Alternating, ItemMarker::Bullet),
            "● ○ ●"
        );
        assert_eq!(
            family_preview(MarkerFamily::Standard, ItemMarker::Numbered),
            "1. a. i."
        );
        assert_eq!(
            family_preview(MarkerFamily::Outline, ItemMarker::Numbered),
            "I. A. 1."
        );
    }

    /// A family with one glyph looks the same at every depth, and previewing it
    /// as "● ● ●" would read as a sequence. Those collapse to a single glyph.
    #[test]
    fn a_single_glyph_family_previews_as_one_glyph() {
        assert_eq!(family_preview(MarkerFamily::Discs, ItemMarker::Bullet), "●");
        assert_eq!(family_preview(MarkerFamily::Rings, ItemMarker::Bullet), "○");
        assert_eq!(family_preview(MarkerFamily::Squares, ItemMarker::Bullet), "▪");
        assert_eq!(family_preview(MarkerFamily::Dashes, ItemMarker::Bullet), "–");
        assert_eq!(
            family_preview(MarkerFamily::Decimal, ItemMarker::Numbered),
            "1."
        );
    }

    /// Every offered family needs a label, or a row renders blank.
    #[test]
    fn every_offered_family_has_a_label() {
        for marker in [ItemMarker::Bullet, ItemMarker::Numbered] {
            for family in MarkerFamily::choices_for(marker) {
                assert!(
                    !family_label(*family).is_empty(),
                    "{family:?} has no label"
                );
            }
        }
    }
}
