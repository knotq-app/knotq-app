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
                .child(knotq_l10n::t("scheme.empty.pick_a_list"))
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
                    .child(knotq_l10n::t("scheme.empty.editor_not_available"))
                    .into_any_element();
            }
        };
        let theme = self.theme();
        let time_format = self.time_format;
        let revision = self.state.content_revision();
        let focused_item = self.selection.focused_item_id;
        let is_renaming = self.rename_node.is_some();
        let remote_cursors = self.remote_cursors_for_scheme(scheme.id);
        editor.update(cx, |ed, cx| {
            ed.sync_from_scheme(&scheme, Some(revision), theme, time_format, window, cx);
            ed.set_remote_cursors(remote_cursors, cx);
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
    crate::frame_log::count(&crate::frame_log::SCROLL_RESTORES);
    window.on_next_frame(move |window, _cx| {
        // The first pass runs before the restored content has been laid out, so
        // `max_offset` may still clamp against a stale height; the second pass
        // corrects that. Neither is worth a frame when the offset is already
        // where it belongs, which is the case whenever a sync round trip
        // changed nothing this view is showing.
        if !restore_scroll_offset(&scroll_handle, offset) {
            return;
        }
        crate::frame_log::count(&crate::frame_log::FORCED_REFRESHES);
        window.refresh();

        let scroll_handle = scroll_handle.clone();
        window.on_next_frame(move |window, _cx| {
            if restore_scroll_offset(&scroll_handle, offset) {
                crate::frame_log::count(&crate::frame_log::FORCED_REFRESHES);
                window.refresh();
            }
        });
    });
}

/// Puts the scroll offset back, reporting whether it actually had to move.
///
/// The caller forces a full window redraw after each pass, so a pass that
/// changes nothing costs a frame for no reason — and a sync round trip arms
/// this restore, which while typing means every round trip. Returning whether
/// anything moved lets the common case cost no frames at all.
fn restore_scroll_offset(scroll_handle: &ScrollHandle, offset: Point<Pixels>) -> bool {
    let max_y = scroll_handle.max_offset().height;
    let y = offset.y.clamp(-max_y, Pixels::ZERO);
    let target = point(offset.x, y);
    if scroll_handle.offset() == target {
        return false;
    }
    scroll_handle.set_offset(target);
    true
}

mod controls;
mod glyph;
mod toolbar;

use self::controls::*;
use self::glyph::*;
