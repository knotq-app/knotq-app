//! Which sidebar look to draw.
//!
//! `Classic` is the shipping sidebar. `Vibrant` is the macOS draft: the *same*
//! sidebar — same width, same rows, same indents, same place — but the window
//! carries a real `NSVisualEffectView` behind it, the sidebar's fill turns
//! translucent so that blur shows through, and each row leads with a filled
//! glyph in the system accent instead of a flat color square.
//!
//! Nothing about the geometry differs between the two, on purpose.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SidebarStyle {
    Classic,
    Vibrant,
}

/// Read once: the look is chosen at launch and never changes for the process,
/// so the row renderers can treat it as a constant.
pub(crate) fn sidebar_style() -> SidebarStyle {
    static STYLE: OnceLock<SidebarStyle> = OnceLock::new();
    *STYLE.get_or_init(|| match std::env::var("KNOTQ_SIDEBAR_STYLE") {
        // `apple` is the name this draft was asked for; keep it as an alias.
        Ok(value)
            if value.eq_ignore_ascii_case("vibrant") || value.eq_ignore_ascii_case("apple") =>
        {
            SidebarStyle::Vibrant
        }
        _ => SidebarStyle::Classic,
    })
}

pub(crate) fn is_vibrant() -> bool {
    sidebar_style() == SidebarStyle::Vibrant
}

/// Does the sidebar own the window's whole left edge — top to bottom, title bar
/// band included — the way Finder's and Mail's do? A vibrant sidebar has to:
/// the blur is a property of the *window*, so a sidebar that stops below the
/// title bar leaves an opaque strip across the top of it, and a sidebar inset
/// as a card draws a lit border right through the middle of the effect.
pub(crate) fn full_height_column() -> bool {
    is_vibrant()
}

/// Vertical room above the first row. A full-height sidebar runs underneath the
/// traffic lights, so it has to clear them.
pub(super) fn content_top_inset() -> f32 {
    if !full_height_column() {
        return 10.0;
    }
    if cfg!(target_os = "macos") {
        44.0
    } else {
        14.0
    }
}

// ---------------------------------------------------------------------------
// Row metrics.
//
// The classic values are the shipping sidebar's and must not move. The vibrant
// ones are Finder's: a taller row, a wider selection pill inset from both
// edges, a deeper indent per level, and a bit more room around the glyph. A
// vibrant sidebar owns the whole window edge, so it can afford the space; the
// classic card cannot.
// ---------------------------------------------------------------------------

fn pick(classic: f32, vibrant: f32) -> f32 {
    if is_vibrant() {
        vibrant
    } else {
        classic
    }
}

/// Width of the navigator column.
pub(crate) fn navigator_width() -> f32 {
    pick(166.0, 196.0)
}

pub(super) fn nav_row_height() -> f32 {
    pick(26.0, 28.0)
}

/// Leading padding of a row's content, before any nesting.
pub(super) fn nav_row_indent_base() -> f32 {
    pick(4.0, 8.0)
}

/// How much one level of nesting shifts a row.
pub(super) fn nav_indent_step() -> f32 {
    pick(9.0, 13.0)
}

pub(super) fn nav_icon_gap() -> f32 {
    pick(7.0, 8.0)
}

/// Corner radius of a row's hover/selection fill.
pub(super) fn nav_row_radius() -> f32 {
    pick(5.0, 7.0)
}

/// Horizontal padding between the sidebar's edge and a row's fill, so the
/// selection reads as a pill rather than a full-width band.
pub(super) fn sidebar_side_padding() -> f32 {
    pick(8.0, 10.0)
}

/// Gap between the pinned group (Calendar, Daily, Archive) and the tree.
pub(super) fn group_gap() -> f32 {
    pick(6.0, 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_opt_in_selects_the_draft() {
        // The default build must be untouched: an unset or unrecognized value
        // is the shipping sidebar, not a half-applied draft.
        assert_eq!(
            sidebar_style(),
            if std::env::var("KNOTQ_SIDEBAR_STYLE")
                .is_ok_and(|v| v.eq_ignore_ascii_case("vibrant") || v.eq_ignore_ascii_case("apple"))
            {
                SidebarStyle::Vibrant
            } else {
                SidebarStyle::Classic
            }
        );
    }
}
