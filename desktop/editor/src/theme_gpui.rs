use gpui::Rgba;
pub use knotq_theme::{token_hsla, token_rgba, Theme};

pub const FONT_UI: &str = "SF Pro Text";
pub const FONT_MONO: &str = "SF Mono";
pub const FONT_SIZE_CAPTION2: f32 = 11.0;
pub const FONT_SIZE_EDITOR: f32 = 14.0;

pub fn text_selection_rgba(theme: Theme) -> Rgba {
    if theme.is_dark {
        token_rgba(0x4f8dff88)
    } else {
        token_rgba(0x1f5fff5c)
    }
}
