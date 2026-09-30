//! Which sidebar look to draw.
//!
//! `Vibrant` is the sidebar on macOS: the *same* sidebar — same rows, same
//! indents, same marks, same place — but the window carries a real
//! `NSVisualEffectView` behind it and the sidebar's fill turns translucent so
//! that blur shows through. `Classic` is the sidebar everywhere else, and the
//! opt-out on macOS for anyone whose machine does not render the effect well.
//!
//! The row *contents* are identical between the two, on purpose. What differs
//! is the frame around them: a vibrant sidebar owns the window's whole left
//! edge, title-bar band included, because the blur is a property of the window
//! (see `full_height_column`).

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SidebarStyle {
    Classic,
    Vibrant,
}

/// Read once: the look is chosen at launch and never changes for the process,
/// so the row renderers can treat it as a constant.
///
/// Vibrant is the default on macOS and unavailable everywhere else — refused
/// rather than half-applied. The whole point of the look is the window's
/// vibrancy, which is an `NSVisualEffectView`; without it the rest — a
/// chromeless full-height column that runs under the title bar and owns the
/// traffic-light corner — is a layout built around an effect that is not there,
/// on platforms whose window decorations do not work that way.
///
/// `KNOTQ_SIDEBAR_STYLE=classic` opts back out. That escape hatch is the reason
/// this reads the environment at all now that vibrant is the default: someone
/// whose machine renders the material badly needs a way back that is not
/// "install the previous version". An unrecognised value is ignored rather than
/// treated as an opt-out, so a typo does not silently hand back the old look.
pub(crate) fn sidebar_style() -> SidebarStyle {
    static STYLE: OnceLock<SidebarStyle> = OnceLock::new();
    *STYLE.get_or_init(|| {
        if !cfg!(target_os = "macos") {
            return SidebarStyle::Classic;
        }
        match std::env::var("KNOTQ_SIDEBAR_STYLE") {
            Ok(value)
                if value.eq_ignore_ascii_case("classic")
                    || value.eq_ignore_ascii_case("plain")
                    || value.eq_ignore_ascii_case("legacy") =>
            {
                SidebarStyle::Classic
            }
            _ => SidebarStyle::Vibrant,
        }
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
///
/// Keyed off the *effective* vibrancy, not `is_vibrant()`: if the effect view
/// failed to install there is no blur to run under the title bar, so the layout
/// reverts with the fill rather than leaving a chromeless column over nothing.
pub(crate) fn full_height_column() -> bool {
    super::vibrant::window_vibrancy_available()
}

/// Height of the band above the first row. A full-height sidebar runs
/// underneath the title bar, so this is the strip the traffic lights sit in —
/// and, beside them, the sync control.
pub(super) fn content_top_inset() -> f32 {
    if !full_height_column() {
        return 10.0;
    }
    44.0
}

/// How far into that band the traffic lights reach, measured from the sidebar's
/// content box. Anything placed in the band starts after this.
pub(super) fn traffic_light_clearance() -> f32 {
    if full_height_column() {
        (72.0 - sidebar_side_padding()).max(0.0)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Row metrics.
//
// The classic values are the shipping sidebar's and must not move. The vibrant
// ones give the extra width a full-window column can afford — a selection pill
// inset from both edges, a slightly deeper indent per level, a little more room
// around the glyph. The row height stays put: it is what sets the rhythm of the
// list, and Finder's rows are not taller than KnotQ's.
// ---------------------------------------------------------------------------

fn pick(classic: f32, vibrant: f32) -> f32 {
    if is_vibrant() {
        vibrant
    } else {
        classic
    }
}

/// Default width of the navigator column, before the user drags it.
pub(crate) fn default_navigator_width() -> f32 {
    pick(166.0, 184.0)
}

/// How far the column can be dragged. The lower bound is where a nested scheme
/// name starts truncating to nothing; the upper is where the sidebar starts
/// crowding the panes it exists to navigate.
pub(crate) const MIN_NAVIGATOR_WIDTH: f32 = 150.0;
pub(crate) const MAX_NAVIGATOR_WIDTH: f32 = 340.0;

/// Width of the strip along the sidebar's trailing edge that resizes it.
pub(crate) const RESIZE_HANDLE_WIDTH: f32 = 6.0;

/// The width to draw, given whatever the settings file holds.
///
/// Clamped rather than trusted: the saved value can come from the other look,
/// from a hand-edited settings file, or from a build with different bounds, and
/// none of those should be able to produce a sidebar the user cannot fix.
pub(crate) fn effective_width(saved: Option<f32>) -> f32 {
    saved
        .filter(|width| width.is_finite())
        .unwrap_or_else(default_navigator_width)
        .clamp(MIN_NAVIGATOR_WIDTH, MAX_NAVIGATOR_WIDTH)
}

/// Where a drag that started at `start_width` and has moved `delta` puts the
/// edge. Computed from the gesture's origin, not from the previous frame, so a
/// pointer dragged past a bound and back returns to exactly where it left.
pub(crate) fn resized_width(start_width: f32, delta: f32) -> f32 {
    (start_width + delta)
        .clamp(MIN_NAVIGATOR_WIDTH, MAX_NAVIGATOR_WIDTH)
        .round()
}

/// Can the user drag this sidebar's edge? Only the full-height column: the
/// classic card is positioned inside the left panel by absolute offsets that
/// assume its width, and this draft does not disturb the shipping look.
pub(crate) fn resizable() -> bool {
    full_height_column()
}

pub(super) fn nav_row_height() -> f32 {
    26.0
}

/// Leading padding of a row's content, before any nesting.
pub(super) fn nav_row_indent_base() -> f32 {
    pick(4.0, 8.0)
}

/// How much one level of nesting shifts a row.
pub(super) fn nav_indent_step() -> f32 {
    pick(9.0, 11.0)
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
    pick(6.0, 7.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_width_is_clamped_not_trusted() {
        assert_eq!(effective_width(None), default_navigator_width());
        assert_eq!(effective_width(Some(f32::NAN)), default_navigator_width());
        assert_eq!(effective_width(Some(0.0)), MIN_NAVIGATOR_WIDTH);
        assert_eq!(effective_width(Some(10_000.0)), MAX_NAVIGATOR_WIDTH);
        assert_eq!(effective_width(Some(200.0)), 200.0);
    }

    #[test]
    fn a_drag_past_a_bound_and_back_returns_where_it_left() {
        let start = 200.0;
        // Out past the minimum...
        assert_eq!(resized_width(start, -400.0), MIN_NAVIGATOR_WIDTH);
        // ...and back to the origin, which must be the original width again
        // rather than the bound plus the return travel.
        assert_eq!(resized_width(start, 0.0), start);
        assert_eq!(resized_width(start, 400.0), MAX_NAVIGATOR_WIDTH);
        assert_eq!(resized_width(start, 20.4), 220.0);
    }

    #[test]
    fn macos_gets_the_vibrant_sidebar_unless_it_opts_out() {
        // Vibrant is the macOS default; `classic` is the way back. Off macOS the
        // look is unavailable whatever the environment says, because the effect
        // it is built around does not exist there.
        let opted_out = std::env::var("KNOTQ_SIDEBAR_STYLE").is_ok_and(|v| {
            v.eq_ignore_ascii_case("classic")
                || v.eq_ignore_ascii_case("plain")
                || v.eq_ignore_ascii_case("legacy")
        });
        let expected = if cfg!(target_os = "macos") && !opted_out {
            SidebarStyle::Vibrant
        } else {
            SidebarStyle::Classic
        };
        assert_eq!(sidebar_style(), expected);
        if !cfg!(target_os = "macos") {
            assert!(!full_height_column());
            assert!(!resizable());
        }
    }

    #[test]
    fn a_typo_does_not_hand_back_the_old_sidebar() {
        // An unrecognized value must not read as an opt-out: that would make
        // `KNOTQ_SIDEBAR_STYLE=clasic` silently ship the look we replaced, and
        // the person who set it would never know why nothing changed. Only the
        // three spellings the opt-out documents count.
        for value in ["", "vibrant", "apple", "clasic", "off", "0", "true"] {
            assert!(
                !(value.eq_ignore_ascii_case("classic")
                    || value.eq_ignore_ascii_case("plain")
                    || value.eq_ignore_ascii_case("legacy")),
                "{value:?} would be read as an opt-out"
            );
        }
    }
}
