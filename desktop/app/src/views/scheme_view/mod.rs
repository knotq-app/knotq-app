use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme, FONT_MONO, FONT_UI};
use gpui::prelude::*;
use gpui::{
    div, point, px, App, ClickEvent, Context, Entity, IntoElement, MouseButton, Pixels, Point,
    ScrollHandle, Window,
};
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
        self.restore_scheme_scroll_after_sync_if_needed(scheme.id, window);

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

    fn restore_scheme_scroll_after_sync_if_needed(
        &mut self,
        scheme_id: knotq_model::SchemeId,
        window: &mut Window,
    ) {
        let Some((pending_scheme_id, offset)) = self.scheme_scroll_restore_after_sync.take() else {
            return;
        };
        if pending_scheme_id != scheme_id {
            return;
        }
        schedule_scroll_offset_restore(self.scheme_scroll_handle.clone(), offset, window);
    }
}

fn schedule_scroll_offset_restore(
    scroll_handle: ScrollHandle,
    offset: Point<Pixels>,
    window: &mut Window,
) {
    window.on_next_frame(move |window, _cx| {
        restore_scroll_offset(&scroll_handle, offset);
        window.refresh();

        let scroll_handle = scroll_handle.clone();
        window.on_next_frame(move |window, _cx| {
            restore_scroll_offset(&scroll_handle, offset);
            window.refresh();
        });
    });
}

fn restore_scroll_offset(scroll_handle: &ScrollHandle, offset: Point<Pixels>) {
    let max_y = scroll_handle.max_offset().height;
    let y = offset.y.clamp(-max_y, Pixels::ZERO);
    scroll_handle.set_offset(point(offset.x, y));
}

mod controls;
mod glyph;
mod toolbar;

use self::controls::*;
use self::glyph::*;
