//! The two things the macOS draft changes about the sidebar: the surface it is
//! painted on, and the mark each row leads with.
//!
//! Nothing here forks a row or moves one. The renderers in `rows.rs` and
//! `trash.rs` remain the single implementation of drag-and-drop, inline rename,
//! the context menus and the layout; they call in here only for a fill and a
//! glyph. The sidebar keeps its width, its row height and its indents.

use super::*;
use gpui::{Hsla, Rgba};

pub(super) const CALENDAR_FILL_ICON: &str = "icons/calendar-fill.svg";
pub(super) const CHECKLIST_FILL_ICON: &str = "icons/checklist-fill.svg";
pub(super) const FOLDER_FILL_ICON: &str = "icons/folder-fill.svg";
pub(super) const TRASH_FILL_ICON: &str = "icons/trash-fill.svg";
pub(super) const DOC_FILL_ICON: &str = "icons/doc-fill.svg";

/// macOS's `controlAccentColor` at its default (blue). The blue glyph column is
/// what makes a sidebar read as an Apple sidebar — the labels and the selection
/// keep carrying the KnotQ theme.
const ACCENT_DARK: u32 = 0x0a84ffff;
const ACCENT_LIGHT: u32 = 0x007affff;

/// Alpha the sidebar tint keeps over the window's vibrancy layer. Fully
/// transparent would let the desktop read straight through and make the labels
/// unreadable over a busy wallpaper; fully opaque would waste the blur.
const VIBRANCY_TINT_ALPHA: f32 = 0.66;

/// Tint for the glyphs that stand for the app's own places — Calendar, Daily,
/// folders, the archive. A scheme keeps its own palette color instead, because
/// in KnotQ that color means something.
pub(super) fn accent(t: Theme) -> Hsla {
    token_hsla(if t.is_dark { ACCENT_DARK } else { ACCENT_LIGHT })
}

/// Is the window drawing a blurred backdrop behind the sidebar? GPUI implements
/// `WindowBackgroundAppearance::Blurred` natively on macOS with a real
/// `NSVisualEffectView`; every other platform keeps an opaque sidebar and an
/// otherwise identical draft.
pub(crate) fn window_vibrancy_available() -> bool {
    cfg!(target_os = "macos") && is_vibrant()
}

/// The sidebar card's fill. Over vibrancy this is a tint, not a background: the
/// blurred desktop supplies most of the color and this only biases it toward
/// the theme and lifts contrast for the labels.
pub(super) fn sidebar_surface(t: Theme) -> Hsla {
    let mut color = token_hsla(t.bg_sidebar);
    if window_vibrancy_available() {
        color.a = VIBRANCY_TINT_ALPHA;
    }
    color
}

/// What a pinned row (Calendar, Daily) leads with: a flat color square in the
/// classic look, the same identity as a filled glyph in the vibrant one.
pub(super) struct RowMark {
    pub icon: &'static str,
    pub square: u32,
    pub tint: Hsla,
}

/// A filled leading glyph, tinted by whatever the row stands for.
pub(super) fn glyph(path: &'static str, color: Hsla) -> gpui::AnyElement {
    Icon::empty()
        .path(path)
        .with_size(px(GLYPH_SIZE))
        .text_color(color)
        .into_any_element()
}

/// A scheme's leading mark. Apple gives every row a glyph and lets its color
/// carry the identity — the way Mail's mailboxes are one symbol in several
/// colors — so a scheme becomes a document glyph in its own palette color
/// rather than a bare square.
pub(super) fn scheme_glyph(color: Rgba) -> gpui::AnyElement {
    glyph(DOC_FILL_ICON, color.into())
}

/// A folder is a solid accent-colored folder whether or not it is open, the way
/// Finder draws one. The classic look keeps swapping open for closed.
pub(super) fn folder_glyph(t: Theme) -> gpui::AnyElement {
    glyph(FOLDER_FILL_ICON, accent(t))
}

/// Glyphs are drawn a shade larger than the color square they replace: a square
/// fills its whole box, a symbol does not.
const GLYPH_SIZE: f32 = 11.5;

/// A selection has to survive the blur behind it, so the vibrant look fills a
/// little harder than the classic one does on an opaque card.
pub(super) fn selected_fill(t: Theme) -> Rgba {
    let mut fill = token_rgba(t.row_selected);
    fill.a = (fill.a * 2.2).min(if t.is_dark { 0.22 } else { 0.30 });
    fill
}

pub(super) fn hover_fill(t: Theme) -> Rgba {
    let mut fill = token_rgba(t.row_hover);
    fill.a = (fill.a * 1.7).min(0.14);
    fill
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tint_is_only_translucent_where_a_blur_backs_it() {
        // An opaque platform must not get a see-through sidebar: with no
        // vibrancy layer behind it, the alpha would composite against whatever
        // the app painted underneath and wash the labels out.
        for t in knotq_theme::all_themes() {
            let color = sidebar_surface(t);
            if window_vibrancy_available() {
                assert!(color.a < 1.0);
            } else {
                assert_eq!(color.a, token_hsla(t.bg_sidebar).a);
            }
        }
    }

    #[test]
    fn selection_stays_inside_its_ceiling() {
        for t in knotq_theme::all_themes() {
            let fill = selected_fill(t);
            assert!(fill.a > token_rgba(t.row_selected).a);
            assert!(fill.a <= 0.30);
        }
    }
}
