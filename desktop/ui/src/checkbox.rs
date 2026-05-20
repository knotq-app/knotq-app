use gpui::prelude::*;
use gpui::{div, px, IntoElement};

use crate::theme::{token_hsla, token_rgba, Theme, FONT_UI};

pub fn task_checkbox(is_checked: bool, theme: Theme) -> gpui::AnyElement {
    let border = if is_checked {
        theme.checkbox_border_on
    } else {
        theme.checkbox_border_off
    };
    let fill = if is_checked {
        theme.checkbox_fill_on
    } else {
        theme.checkbox_fill_off
    };

    div()
        .w(px(15.0))
        .h(px(15.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(token_hsla(border))
        .bg(token_rgba(fill))
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_size(px(10.0))
        .line_height(px(15.0))
        .text_color(token_hsla(theme.checkbox_mark))
        .child(if is_checked { "✓" } else { "" })
        .into_any_element()
}
