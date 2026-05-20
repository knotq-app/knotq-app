use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme, FONT_MONO, FONT_UI};
use gpui::prelude::*;
use gpui::{div, px, App, ClickEvent, Context, Entity, IntoElement, MouseButton, Window};
use gpui_component::scroll::Scrollbar;
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, Sizable};
use knotq_editor::SchemeEditor;

impl KnotQApp {
    pub fn render_scheme_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme();
        let Some(scheme) = self.current_scheme().cloned() else {
            return div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(token_hsla(t.text_muted))
                .child("Pick a list on the left")
                .into_any_element();
        };

        let editor = match self.ensure_scheme_editor(window, cx) {
            Some(ed) => ed,
            None => {
                return div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(token_hsla(t.text_muted))
                    .child("Editor not available")
                    .into_any_element();
            }
        };
        let theme = self.theme();
        let time_format = self.time_format;
        let focused_item = self.selection.focused_item_id;
        let is_renaming = self.rename_node.is_some();
        editor.update(cx, |ed, cx| {
            ed.sync_from_scheme(scheme.clone(), theme, time_format, window, cx);
            if let Some(item_id) = focused_item.filter(|_| !is_renaming) {
                ed.focus_item(item_id, window, cx);
            }
        });
        if focused_item.is_some() {
            self.selection.focused_item_id = None;
        }
        if editor.read(cx).needs_cursor_scroll() {
            let editor = editor.clone();
            window.defer(cx, move |_window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.scroll_to_cursor(cx);
                });
            });
        }
        let toolbar = self.render_scheme_toolbar(&scheme, editor.clone(), cx);

        div()
            .relative()
            .flex_1()
            .h_full()
            .bg(token_hsla(t.bg_app))
            .child(
                div()
                    .id("scheme-editor-scroll-shell")
                    .relative()
                    .h_full()
                    .min_h_0()
                    .child(
                        div()
                            .id("scheme-editor-scroll")
                            .h_full()
                            .min_h_0()
                            .track_scroll(&self.scheme_scroll_handle)
                            .overflow_y_scroll()
                            .child(div().relative().w_full().child(editor)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .child(
                                Scrollbar::vertical(&self.scheme_scroll_handle)
                                    .id("scheme-editor-scrollbar"),
                            ),
                    ),
            )
            .child(toolbar)
            .into_any_element()
    }
}

mod controls;
mod glyph;
mod toolbar;

use self::controls::*;
use self::glyph::*;
