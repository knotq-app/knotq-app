//! The one thing the macOS draft changes about the sidebar: the surface it is
//! painted on.
//!
//! The sidebar's contents are untouched — same width, same rows, same marks,
//! same indents, same place. Under vibrancy the card's fill simply stops being
//! opaque so the window's blur shows through it.

use super::*;
use gpui::Hsla;

/// How much of the theme's own sidebar color is laid over the vibrancy.
///
/// Zero, like Finder: the `sidebar` material is already a legible frosted
/// surface, and every bit of tint laid over it is blur traded away. The knob
/// stays because KnotQ's themes are not all grey and a future one may want its
/// hue back — but the default is to let the material do the work.
const VIBRANCY_TINT_ALPHA: f32 = 0.0;

/// Is the window drawing a blurred backdrop behind the sidebar? GPUI implements
/// `WindowBackgroundAppearance::Blurred` natively on macOS with a real
/// `NSVisualEffectView`; every other platform keeps an opaque sidebar.
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
}
