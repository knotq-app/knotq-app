//! Adapter from `knotq_theme` palette values to GPUI types.
//!
//! Theme tokens are stored as 0xRRGGBBAA `u32`s. The scheme palette
//! ([`knotq_theme::PALETTE`]) is stored as 0xRRGGBB; use [`palette_hsla`]
//! to add an alpha and produce an [`Hsla`].

use chrono::{DateTime, Local};
use gpui::{Hsla, Rgba};
pub use knotq_theme::{all_themes, scheme_color, token_hsla, token_rgba, Theme, PALETTE};

pub const FONT_UI: &str = "SF Pro Text";
pub const FONT_DISPLAY: &str = "SF Pro Display";
pub const FONT_MONO: &str = "SF Mono";

pub const FONT_SIZE_BODY: f32 = 13.0;
pub const FONT_SIZE_HEADLINE: f32 = 13.0;
pub const FONT_SIZE_CAPTION2: f32 = 11.0;
pub const FONT_SIZE_CALENDAR_ITEM: f32 = 11.0;
pub const FONT_SIZE_CALENDAR_TIME: f32 = 8.8;

pub fn selected_date_text_color(t: Theme) -> u32 {
    let selected_bg = t.caret_color;
    let preferred = t.text_highlight;
    if contrast_with_bg(preferred, selected_bg) >= MIN_TEXT_CONTRAST {
        return preferred;
    }

    let alternate = t.bg_modal;
    if contrast_with_bg(alternate, selected_bg) >= MIN_TEXT_CONTRAST {
        return alternate;
    }

    [t.text_primary, 0xffffffff, 0x000000ff]
        .into_iter()
        .max_by(|a, b| {
            contrast_with_bg(*a, selected_bg)
                .partial_cmp(&contrast_with_bg(*b, selected_bg))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(preferred)
}

/// Convert a 0xRRGGBB palette color into [`Rgba`] with the given alpha.
pub fn palette_rgba(rgb: u32, alpha: f32) -> Rgba {
    Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.0,
        g: ((rgb >> 8) & 0xff) as f32 / 255.0,
        b: (rgb & 0xff) as f32 / 255.0,
        a: alpha,
    }
}

pub fn palette_hsla(rgb: u32, alpha: f32) -> Hsla {
    palette_rgba(rgb, alpha).into()
}

fn swiftui_saturation(color: Rgba, amount: f32) -> Rgba {
    let luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;
    let mix = |channel: f32| (luminance + (channel - luminance) * amount).clamp(0.0, 1.0);
    Rgba {
        r: mix(color.r),
        g: mix(color.g),
        b: mix(color.b),
        a: color.a,
    }
}

const MIN_TEXT_CONTRAST: f32 = 4.5;

fn contrast_with_bg(fg: u32, bg: u32) -> f32 {
    let fg = composite_token_over_bg(fg, bg);
    let bg = token_rgb(bg);
    contrast_ratio(fg, bg)
}

fn composite_token_over_bg(fg: u32, bg: u32) -> [f32; 3] {
    let [fr, fg_channel, fb] = token_rgb(fg);
    let [br, bg_channel, bb] = token_rgb(bg);
    let alpha = (fg & 0xff) as f32 / 255.0;
    [
        fr * alpha + br * (1.0 - alpha),
        fg_channel * alpha + bg_channel * (1.0 - alpha),
        fb * alpha + bb * (1.0 - alpha),
    ]
}

fn token_rgb(c: u32) -> [f32; 3] {
    [
        ((c >> 24) & 0xff) as f32 / 255.0,
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
    ]
}

fn contrast_ratio(a: [f32; 3], b: [f32; 3]) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

fn relative_luminance(rgb: [f32; 3]) -> f32 {
    fn linear(channel: f32) -> f32 {
        if channel <= 0.03928 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}

/// Color for an item rendered on the calendar, matching old KnotQ's
/// `foregroundStyle(color).saturation(...)` behavior.
pub fn calendar_item_color(is_done: bool, color_index: u8, is_dark: bool) -> Hsla {
    let rgb = scheme_color(color_index, is_dark);
    let amount = if is_done {
        if is_dark {
            0.35
        } else {
            0.45
        }
    } else {
        if is_dark {
            0.7
        } else {
            0.9
        }
    };
    let mut hsla: Hsla = swiftui_saturation(palette_rgba(rgb, 1.0), amount).into();
    if is_done {
        hsla.a *= 0.78;
    }
    hsla
}

/// Old KnotQ desaturated scheme colors in the assignments/reminders list.
pub fn upcoming_scheme_color(color_index: u8, is_dark: bool) -> Hsla {
    let rgb = scheme_color(color_index, is_dark);
    let mut hsla: Hsla = palette_hsla(rgb, 1.0);
    hsla.s *= 0.72;
    hsla
}

pub fn date_status_color(dt: DateTime<Local>, default: Hsla) -> Hsla {
    let now = Local::now();
    let light_surface = default.l < 0.55;
    if dt < now {
        return token_hsla(if light_surface {
            0xd20f39ff
        } else {
            0xff5a53ff
        });
    }

    let today = now.date_naive();
    let day_diff = (dt.date_naive() - today).num_days();
    if day_diff <= 0 {
        token_hsla(if light_surface {
            0x2f67cfff
        } else {
            0xbfbfffff
        })
    } else if day_diff <= 1 {
        token_hsla(if light_surface {
            0x4f5f8fff
        } else {
            0xe5e5ffff
        })
    } else {
        default
    }
}

pub fn event_status_color(
    start: DateTime<Local>,
    end: Option<DateTime<Local>>,
    default: Hsla,
) -> Hsla {
    let now = Local::now();
    if end.is_some_and(|end| start <= now && end > now) {
        return upcoming_today_color(default);
    }

    date_status_color(start, default)
}

fn upcoming_today_color(default: Hsla) -> Hsla {
    let light_surface = default.l < 0.55;
    token_hsla(if light_surface {
        0x2f67cfff
    } else {
        0xbfbfffff
    })
}

/// Helper: scheme square color in the sidebar.
pub fn scheme_square_color(color_index: u8, is_dark: bool) -> Rgba {
    palette_rgba(scheme_color(color_index, is_dark), 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn selected_date_text_color_has_readable_contrast() {
        for theme in [
            knotq_theme::theme_obsidian(),
            knotq_theme::theme_rose_pine_moon(),
            knotq_theme::theme_catppuccin_mocha(),
            knotq_theme::theme_tokyo_night(),
            knotq_theme::theme_light(),
            knotq_theme::theme_parchment(),
            knotq_theme::theme_rose_pine_dawn(),
            knotq_theme::theme_catppuccin_latte(),
        ] {
            let text = selected_date_text_color(theme);
            assert!(
                contrast_with_bg(text, theme.caret_color) >= MIN_TEXT_CONTRAST,
                "{} selected date text contrast was too low",
                theme.name
            );
        }
    }

    #[test]
    fn event_status_color_marks_ongoing_events_as_today_not_past() {
        let default = token_hsla(0xe8edf2e6);
        let now = Local::now();

        assert_eq!(
            event_status_color(
                now - Duration::minutes(10),
                Some(now + Duration::minutes(10)),
                default
            ),
            token_hsla(0xbfbfffff)
        );
        assert_eq!(
            event_status_color(
                now - Duration::minutes(20),
                Some(now - Duration::minutes(10)),
                default
            ),
            token_hsla(0xff5a53ff)
        );
    }
}
