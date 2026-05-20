use gpui::{Hsla, Rgba};
pub use knotq_theme::Theme;

pub const FONT_UI: &str = "SF Pro Text";
pub const FONT_MONO: &str = "SF Mono";
pub const FONT_SIZE_CAPTION2: f32 = 11.0;
pub const FONT_SIZE_EDITOR: f32 = 14.0;

pub fn token_rgba(c: u32) -> Rgba {
    Rgba {
        r: ((c >> 24) & 0xff) as f32 / 255.0,
        g: ((c >> 16) & 0xff) as f32 / 255.0,
        b: ((c >> 8) & 0xff) as f32 / 255.0,
        a: (c & 0xff) as f32 / 255.0,
    }
}

pub fn token_hsla(c: u32) -> Hsla {
    token_rgba(c).into()
}

pub fn text_selection_rgba(theme: Theme) -> Rgba {
    if theme.is_dark {
        token_rgba(0x4f8dff88)
    } else {
        token_rgba(0x1f5fff5c)
    }
}
