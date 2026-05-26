//! Theme palette and GPUI color conversion utilities.

mod dark;
mod light;
pub mod palette;

use gpui::{Hsla, Rgba};

pub use dark::*;
pub use light::*;
pub use palette::*;

/// A color stored as 0xRRGGBBAA. Always include alpha; use `0xRRGGBBff` for opaque.
pub type Color = u32;

pub(crate) const fn rgb(hex: u32) -> Color {
    (hex << 8) | 0xff
}

pub(crate) const fn rgba(hex: u32) -> Color {
    hex
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub is_dark: bool,
    pub bg_app: Color,
    pub bg_sidebar: Color,
    pub bg_toolbar: Color,
    pub bg_upcoming: Color,
    pub bg_hint: Color,
    pub bg_cal_hdr: Color,
    pub bg_modal: Color,
    pub border_main: Color,
    pub border_soft: Color,
    pub border_strong: Color,
    pub border_overlay: Color,
    pub text_primary: Color,
    pub text_dim: Color,
    pub text_muted: Color,
    pub text_soft: Color,
    pub text_highlight: Color,
    pub text_placeholder: Color,
    pub text_today: Color,
    pub caret_color: Color,
    pub row_alt: Color,
    pub row_hover: Color,
    pub row_hover_strong: Color,
    pub row_selected: Color,
    pub button_bg: Color,
    pub button_hover: Color,
    pub divider: Color,
    pub divider_soft: Color,
    pub divider_faint: Color,
    pub divider_tiny: Color,
    pub overlay_scrim: Color,
    pub cal_grid: Color,
    pub cal_grid_soft: Color,
    pub cal_past: Color,
    pub event_bg: Color,
    pub event_border: Color,
    pub checkbox_border_on: Color,
    pub checkbox_border_off: Color,
    pub checkbox_fill_on: Color,
    pub checkbox_fill_off: Color,
    pub checkbox_mark: Color,
    pub done_text: Color,
    pub drag_preview_bg: Color,
    pub toolbar_chip_bg: Color,
    pub toolbar_chip_border: Color,
    pub toolbar_chip_selected_text: Color,
    pub toolbar_chip_muted: Color,
    pub toolbar_chip_separator: Color,
    pub daily_title_active: Color,
    pub daily_title_muted: Color,
    pub link: Color,
    pub link_hover: Color,
    pub cal_event_text: Color,
    pub cal_weekend_tint: Color,
    pub row_stripe: Color,
}

pub fn all_themes() -> [Theme; 2] {
    [theme_obsidian(), theme_light()]
}

/// Convert a packed 0xRRGGBBAA token into [`Rgba`].
pub fn token_rgba(c: u32) -> Rgba {
    Rgba {
        r: ((c >> 24) & 0xff) as f32 / 255.0,
        g: ((c >> 16) & 0xff) as f32 / 255.0,
        b: ((c >> 8) & 0xff) as f32 / 255.0,
        a: (c & 0xff) as f32 / 255.0,
    }
}

/// Convert a packed 0xRRGGBBAA token into [`Hsla`].
pub fn token_hsla(c: u32) -> Hsla {
    token_rgba(c).into()
}
