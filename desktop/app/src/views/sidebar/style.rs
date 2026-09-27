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
