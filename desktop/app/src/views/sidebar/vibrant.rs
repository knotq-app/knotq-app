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
/// The material alone is grey, and KnotQ's themes are not: without a tint the
/// sidebar stops belonging to the theme the rest of the window is painted in.
/// This is the balance point — enough of `bg_sidebar` to carry the hue, little
/// enough that the blur behind it still reads. `KNOTQ_SIDEBAR_TINT` overrides
/// it with a 0..1 alpha, since where that balance sits is a matter of taste.
const DEFAULT_VIBRANCY_TINT_ALPHA: f32 = 0.30;

fn vibrancy_tint_alpha() -> f32 {
    std::env::var("KNOTQ_SIDEBAR_TINT")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|alpha| alpha.is_finite())
        .map(|alpha| alpha.clamp(0.0, 1.0))
        .unwrap_or(DEFAULT_VIBRANCY_TINT_ALPHA)
}

/// Is the window drawing a blurred backdrop behind the sidebar? True exactly
/// when the vibrant look is active, which `sidebar_style` already restricts to
/// macOS — the effect is an `NSVisualEffectView` and has no equivalent
/// elsewhere.
pub(crate) fn window_vibrancy_available() -> bool {
    is_vibrant()
}

/// The sidebar card's fill. Over vibrancy this is a tint, not a background: the
/// blurred desktop supplies most of the color and this only biases it toward
/// the theme and lifts contrast for the labels.
pub(super) fn sidebar_surface(t: Theme) -> Hsla {
    let mut color = token_hsla(t.bg_sidebar);
    if window_vibrancy_available() {
        color.a = vibrancy_tint_alpha();
    }
    color
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tint_alpha_is_always_a_usable_alpha() {
        // Whatever the environment says, this ends up as a color's alpha, so a
        // typo, a negative, or an infinity must not reach the renderer.
        for value in ["", "nope", "-3", "9", "inf", "NaN", "0.5"] {
            // SAFETY: single-threaded test, and the variable is read nowhere
            // else while this runs.
            unsafe { std::env::set_var("KNOTQ_SIDEBAR_TINT", value) };
            let alpha = vibrancy_tint_alpha();
            assert!(
                (0.0..=1.0).contains(&alpha),
                "{value:?} produced alpha {alpha}"
            );
        }
        unsafe { std::env::remove_var("KNOTQ_SIDEBAR_TINT") };
        assert_eq!(vibrancy_tint_alpha(), DEFAULT_VIBRANCY_TINT_ALPHA);
    }

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
