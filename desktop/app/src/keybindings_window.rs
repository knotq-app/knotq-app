use gpui::prelude::*;
use gpui::{
    div, px, size, AnyWindowHandle, App, Context, FocusHandle, Global, IntoElement, Render,
    Window, WindowBounds, WindowOptions,
};

use crate::theme_gpui::{token_hsla, Theme, FONT_UI};

#[derive(Default)]
struct KeybindingsWindowHandle(Option<AnyWindowHandle>);

impl Global for KeybindingsWindowHandle {}

pub struct KeybindingsWindow {
    focus_handle: FocusHandle,
    theme: Theme,
}

impl KeybindingsWindow {
    pub fn open(theme: Theme, cx: &mut App) {
        if !cx.has_global::<KeybindingsWindowHandle>() {
            cx.set_global(KeybindingsWindowHandle::default());
        }
        if let Some(handle) = cx.global::<KeybindingsWindowHandle>().0 {
            if handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
                return;
            }
        }

        let window_size = size(px(560.0), px(620.0));
        let options = WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Keyboard Shortcuts".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::centered(window_size, cx)),
            window_min_size: Some(size(px(420.0), px(360.0))),
            focus: true,
            ..Default::default()
        };
        if let Ok(handle) = cx.open_window(options, move |_window, cx| {
            cx.new(|cx| KeybindingsWindow {
                focus_handle: cx.focus_handle(),
                theme,
            })
        }) {
            cx.update_global::<KeybindingsWindowHandle, _>(|state, _| state.0 = Some(handle.into()));
        }
    }
}

const BINDINGS: &[(&str, &[(&str, &str)])] = &[
    (
        "Editing",
        &[
            ("Undo / redo", "Cmd/Ctrl+Z / Shift+Cmd/Ctrl+Z"),
            ("Copy / cut / paste", "Cmd/Ctrl+C / X / V"),
            ("Move by word", "Option+Arrow (macOS) / Ctrl+Arrow"),
            ("Move to line start / end", "Home / End or Cmd/Ctrl+Arrow"),
            ("Select to line start / end", "Shift+Home / End"),
            ("Indent / unindent", "Tab / Shift+Tab"),
        ],
    ),
    (
        "KnotQ",
        &[
            ("Search workspace", "Cmd/Ctrl+F"),
            ("New scheme / folder", "Cmd/Ctrl+N / Shift+Cmd/Ctrl+N"),
            ("Calendar / Daily Queue", "Cmd/Ctrl+U / Cmd/Ctrl+D"),
            ("Settings", "Cmd/Ctrl+,"),
            ("Toggle markers", "Cmd/Ctrl+1 / 2 / 3 / 4"),
            ("Format text", "Cmd/Ctrl+B / I / Shift+X / J"),
        ],
    ),
];

impl Render for KeybindingsWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .size_full()
            .font_family(FONT_UI)
            .bg(token_hsla(theme.bg_app))
            .text_color(token_hsla(theme.text_primary))
            .key_context("keybindings-window")
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .id("keybindings-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .p(px(20.0))
                    .flex()
                    .flex_col()
                    .gap(px(20.0))
                    .child(div().text_size(px(20.0)).font_weight(gpui::FontWeight::SEMIBOLD).child("Keyboard Shortcuts"))
                    .child(div().text_size(px(12.0)).text_color(token_hsla(theme.text_muted)).child("Shortcuts use your platform's primary modifier."))
                    .children(BINDINGS.iter().map(|(title, bindings)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(7.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(*title),
                            )
                            .children(bindings.iter().map(|(action, shortcut)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(16.0))
                                    .child(div().flex_1().text_size(px(12.0)).child(*action))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(token_hsla(theme.text_muted))
                                            .child(*shortcut),
                                    )
                            }))
                    })),
            )
    }
}
