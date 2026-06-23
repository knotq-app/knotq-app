use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement, MouseButton, Window};

use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

use super::LINUX_WINDOW_CONTROLS_W;

impl KnotQApp {
    pub(super) fn uses_linux_client_decorations() -> bool {
        cfg!(target_os = "linux")
    }

    pub(super) fn render_linux_window_controls(
        &self,
        window: &mut Window,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !Self::uses_linux_client_decorations() {
            return None;
        }

        let controls = window.window_controls();
        Some(
            div()
                .id("linux-window-controls")
                .absolute()
                .top_0()
                .right_0()
                .h_full()
                .w(px(LINUX_WINDOW_CONTROLS_W))
                .flex()
                .items_center()
                .justify_end()
                .bg(token_rgba(t.bg_cal_hdr))
                .child(Self::linux_window_control_button(
                    "linux-window-minimize",
                    Self::linux_minimize_glyph(t),
                    false,
                    controls.minimize,
                    |_: &ClickEvent, window, _cx| window.minimize_window(),
                    t,
                ))
                .child(Self::linux_window_control_button(
                    "linux-window-maximize",
                    Self::linux_maximize_glyph(t),
                    false,
                    controls.maximize,
                    |_: &ClickEvent, window, _cx| window.zoom_window(),
                    t,
                ))
                .child(Self::linux_window_control_button(
                    "linux-window-close",
                    Self::linux_close_glyph(),
                    true,
                    true,
                    cx.listener(|this, _: &ClickEvent, window, _cx| {
                        this.flush_for_shutdown("linux title bar close");
                        window.remove_window();
                    }),
                    t,
                ))
                .into_any_element(),
        )
    }

    fn linux_window_control_button(
        id: &'static str,
        glyph: gpui::AnyElement,
        is_close: bool,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
        t: Theme,
    ) -> gpui::AnyElement {
        let hover_bg = if is_close {
            if t.is_dark {
                0xff5a537d
            } else {
                0xd20f3988
            }
        } else {
            t.button_hover
        };

        div()
            .id(id)
            .w(px(44.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .text_size(px(12.0))
            .text_color(token_hsla(if is_close {
                t.text_primary
            } else {
                t.text_dim
            }))
            .when(enabled, |s| {
                s.cursor_pointer()
                    .hover(move |h| h.bg(token_rgba(hover_bg)))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(move |event: &ClickEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        on_click(event, window, cx);
                    })
            })
            .when(!enabled, |s| s.opacity(0.35))
            .child(glyph)
            .into_any_element()
    }

    fn linux_minimize_glyph(t: Theme) -> gpui::AnyElement {
        div()
            .w(px(10.0))
            .h(px(1.5))
            .rounded(px(1.0))
            .bg(token_rgba(t.text_dim))
            .into_any_element()
    }

    fn linux_maximize_glyph(t: Theme) -> gpui::AnyElement {
        div()
            .w(px(9.0))
            .h(px(9.0))
            .rounded(px(1.5))
            .border_1()
            .border_color(token_rgba(t.text_dim))
            .into_any_element()
    }

    fn linux_close_glyph() -> gpui::AnyElement {
        div().child("x").into_any_element()
    }
}
