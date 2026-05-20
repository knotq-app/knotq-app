use gpui::{Hsla, Rgba};
pub use knotq_theme::Theme;

pub const FONT_UI: &str = "SF Pro Text";

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
