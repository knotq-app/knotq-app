use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, Decorations, IntoElement, MouseButton, Window};
use gpui_component::tooltip::Tooltip;

use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

use super::LINUX_WINDOW_CONTROLS_W;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxZoomControl {
    Maximize,
    Restore,
}

fn has_client_decorations(decorations: Decorations) -> bool {
    matches!(decorations, Decorations::Client { .. })
}

fn linux_zoom_control(is_maximized: bool) -> LinuxZoomControl {
    if is_maximized {
        LinuxZoomControl::Restore
    } else {
        LinuxZoomControl::Maximize
    }
}

fn linux_zoom_control_label(control: LinuxZoomControl) -> &'static str {
    match control {
        LinuxZoomControl::Maximize => "Maximize window",
        LinuxZoomControl::Restore => "Restore window",
    }
}

impl KnotQApp {
    pub(super) fn uses_linux_client_decorations(window: &Window) -> bool {
        cfg!(target_os = "linux") && has_client_decorations(window.window_decorations())
    }

    pub(super) fn render_linux_window_controls(
        &self,
        window: &mut Window,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !Self::uses_linux_client_decorations(window) {
            return None;
        }

        let controls = window.window_controls();
        let zoom_control = linux_zoom_control(window.is_maximized());
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
                    "Minimize window",
                    Self::linux_minimize_glyph(t),
                    false,
                    controls.minimize,
                    |_: &ClickEvent, window, _cx| window.minimize_window(),
                    t,
                ))
                .child(Self::linux_window_control_button(
                    "linux-window-maximize",
                    linux_zoom_control_label(zoom_control),
                    Self::linux_zoom_glyph(zoom_control, t),
                    false,
                    controls.maximize,
                    |_: &ClickEvent, window, _cx| window.zoom_window(),
                    t,
                ))
                .child(Self::linux_window_control_button(
                    "linux-window-close",
                    "Close window",
                    Self::linux_close_glyph(),
                    true,
                    true,
                    cx.listener(|_this, _: &ClickEvent, window, _cx| {
                        window.remove_window();
                    }),
                    t,
                ))
                .into_any_element(),
        )
    }

    fn linux_window_control_button(
        id: &'static str,
        tooltip: &'static str,
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
            .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
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

    fn linux_zoom_glyph(control: LinuxZoomControl, t: Theme) -> gpui::AnyElement {
        match control {
            LinuxZoomControl::Maximize => div()
                .w(px(9.0))
                .h(px(9.0))
                .rounded(px(1.5))
                .border_1()
                .border_color(token_rgba(t.text_dim))
                .into_any_element(),
            LinuxZoomControl::Restore => div()
                .relative()
                .w(px(11.0))
                .h(px(11.0))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(1.0))
                        .border_1()
                        .border_color(token_rgba(t.text_dim)),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(1.0))
                        .border_1()
                        .border_color(token_rgba(t.text_dim))
                        .bg(token_rgba(t.bg_cal_hdr)),
                )
                .into_any_element(),
        }
    }

    fn linux_close_glyph() -> gpui::AnyElement {
        div().child("x").into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Tiling;

    #[test]
    fn client_controls_follow_runtime_decoration_mode() {
        assert!(!has_client_decorations(Decorations::Server));
        assert!(has_client_decorations(Decorations::Client {
            tiling: Tiling::default(),
        }));
    }

    #[test]
    fn maximized_window_uses_restore_control() {
        assert_eq!(linux_zoom_control(false), LinuxZoomControl::Maximize);
        assert_eq!(linux_zoom_control(true), LinuxZoomControl::Restore);
        assert_eq!(
            linux_zoom_control_label(LinuxZoomControl::Maximize),
            "Maximize window"
        );
        assert_eq!(
            linux_zoom_control_label(LinuxZoomControl::Restore),
            "Restore window"
        );
    }
}
