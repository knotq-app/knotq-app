use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement, MouseButton, Window, WindowControlArea};
use gpui_component::{Icon, IconName, Sizable};
use knotq_commands::Command;
use knotq_l10n::t as tr;
use knotq_model::SchemeId;
use knotq_storage_json::CalendarViewMode;

use crate::app::{daily_queue_marker_color, KnotQApp, View};
use crate::theme_gpui::{
    palette_hsla, scheme_color, token_hsla, token_rgba, Theme, PALETTE, FONT_SIZE_HEADLINE,
};
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

mod search;
#[cfg(feature = "accounts")]
mod sync_control;
mod update_control;
mod window_controls;

const TITLE_CONTENT_W: f32 = 430.0;
const LINUX_TITLE_CONTENT_W: f32 = 340.0;
const LINUX_WINDOW_CONTROLS_W: f32 = 132.0;
const TITLE_MARKER_SIZE: f32 = 18.0;
const TITLE_TEXT_W: f32 = 190.0;
const LINUX_TITLE_TEXT_W: f32 = 150.0;
// macOS renders native traffic-light controls at the top-left, so the title bar
// reserves room for them. Other platforms have no left-side window controls, so
// they fall back to the normal edge padding instead of leaving dead space.
const MACOS_TRAFFIC_LIGHT_PAD: f32 = 80.0;
const TITLE_EDGE_PAD: f32 = 16.0;

// Semantic sync status colors (the theme has no status palette of its own).
#[cfg(feature = "accounts")]
pub(crate) const STATUS_OK: u32 = 0x22c55eff;
#[cfg(feature = "accounts")]
pub(crate) const STATUS_SYNCING: u32 = 0x3b82f6ff;
#[cfg(feature = "accounts")]
pub(crate) const STATUS_PENDING: u32 = 0xf59e0bff;
#[cfg(feature = "accounts")]
pub(crate) const STATUS_ERROR: u32 = 0xef4444ff;

impl KnotQApp {
    pub(crate) fn render_title_bar(
        &mut self,
        window: &mut Window,
        view: View,
        title: String,
        scheme: Option<(SchemeId, String, u8)>,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let linux_client_decorations = Self::uses_linux_client_decorations(window);
        let title_content_w = if linux_client_decorations {
            LINUX_TITLE_CONTENT_W
        } else {
            TITLE_CONTENT_W
        };
        let title_text_w = if linux_client_decorations {
            LINUX_TITLE_TEXT_W
        } else {
            TITLE_TEXT_W
        };

        let left_pad = if cfg!(target_os = "macos") {
            MACOS_TRAFFIC_LIGHT_PAD
        } else {
            TITLE_EDGE_PAD
        };

        let base = div()
            .relative()
            .flex()
            .items_center()
            .h(px(38.0))
            .pl(px(left_pad))
            .pr(px(TITLE_EDGE_PAD))
            .bg(token_hsla(t.bg_cal_hdr))
            .border_b_1()
            .border_color(token_rgba(t.divider))
            .when(linux_client_decorations, |s| {
                s.pr(px(16.0 + LINUX_WINDOW_CONTROLS_W))
            });

        let active_scheme = scheme.as_ref().filter(|_| view == View::Scheme);
        let marker_color = if let Some((_, _, color_index)) = active_scheme {
            palette_hsla(scheme_color(*color_index, t.is_dark), 1.0)
        } else if view == View::Union {
            token_hsla(t.text_highlight)
        } else if view == View::DailyQueue {
            token_hsla(daily_queue_marker_color(t.is_dark))
        } else {
            token_hsla(t.text_dim)
        };

        let marker = if let Some((scheme_id, _, _color_index)) = active_scheme {
            div()
                .id("title-scheme-color")
                .rounded(px(7.0))
                .bg(token_rgba(t.button_bg))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .px(px(6.0))
                .py(px(4.0))
                .hover({
                    let hover = t.button_hover;
                    move |s| s.bg(token_rgba(hover))
                })
                .on_click({
                    let scheme_id = *scheme_id;
                    cx.listener(move |this, event: &ClickEvent, _window, cx| {
                        this.scheme_color_popover = Some((event.position(), scheme_id));
                        cx.notify();
                    })
                })
                .child(
                    div()
                        .w(px(TITLE_MARKER_SIZE))
                        .h(px(TITLE_MARKER_SIZE))
                        .rounded(px(3.0))
                        .bg(marker_color),
                )
                .into_any_element()
        } else {
            div()
                .w(px(TITLE_MARKER_SIZE))
                .h(px(TITLE_MARKER_SIZE))
                .rounded(px(3.0))
                .bg(marker_color)
                .into_any_element()
        };

        let mut calendar_mode_controls: Vec<gpui::AnyElement> = Vec::new();
        if view == View::Union {
            for (i, (label, mode)) in [
                (tr("titlebar.calendar_mode.week"), CalendarViewMode::Week),
                (tr("titlebar.calendar_mode.month"), CalendarViewMode::Month),
            ]
            .into_iter()
            .enumerate()
            {
                let is_active = self.calendar_view == mode;
                calendar_mode_controls.push(
                    div()
                        .id(("title-calendar-mode", i))
                        .h_full()
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .font_weight(if is_active {
                            gpui::FontWeight::SEMIBOLD
                        } else {
                            gpui::FontWeight::NORMAL
                        })
                        .text_color(token_hsla(if is_active {
                            t.text_primary
                        } else {
                            t.text_muted
                        }))
                        .when(is_active, {
                            let c = t.row_selected;
                            move |s| s.bg(token_rgba(c))
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.set_calendar_view(mode, cx);
                        }))
                        .child(label)
                        .into_any_element(),
                );
            }
        }

        let search_control = self.render_title_bar_search(window, t, cx);
        let sync_control = self.render_title_bar_sync_control(t, cx);
        let update_control = self.render_title_bar_update_control(t, cx);

        base.child(
            div()
                .id("title-drag-region")
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right_0()
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(WindowControlArea::Drag)
                // A custom title bar should retain the platform convention:
                // double-clicking its empty area toggles zoom/maximize.
                .on_click(|event, window, cx| {
                    cx.stop_propagation();
                    if event.click_count() == 2 {
                        window.zoom_window();
                    } else if cfg!(target_os = "linux") && event.is_right_click() {
                        window.show_window_menu(event.position());
                    }
                })
                .when(linux_client_decorations, |s| {
                    s.on_mouse_down(MouseButton::Left, |_, window, cx| {
                        cx.stop_propagation();
                        window.start_window_move();
                    })
                })
                .child(
                    div()
                        .w(px(title_content_w))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .child(marker)
                        .child(
                            div()
                                .w(px(title_text_w))
                                .min_w_0()
                                .truncate()
                                .text_size(px(FONT_SIZE_HEADLINE))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(token_hsla(t.text_primary))
                                .child(title),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .gap(px(8.0))
                .when_some(sync_control, |s, sync_control| s.child(sync_control))
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .window_control_area(WindowControlArea::Drag),
                ),
        )
        .child(div().w(px(title_content_w)).flex_shrink_0().h_full())
        .child(
            div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .when(!calendar_mode_controls.is_empty(), move |s| {
                    s.child(
                        div()
                            .h(px(26.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(token_rgba(t.border_soft))
                            .bg(token_rgba(t.button_bg))
                            .p(px(2.0))
                            .flex()
                            .items_center()
                            .children(calendar_mode_controls),
                    )
                })
                .when_some(update_control, |s, update_control| s.child(update_control))
                .child(search_control)
                .child(
                    div()
                        .id("title-settings")
                        .h(px(26.0))
                        .w(px(28.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(token_rgba(t.border_soft))
                        .bg(token_rgba(t.button_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover({
                            let c = t.button_hover;
                            move |s| s.bg(token_rgba(c))
                        })
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.open_settings(cx);
                            this.focus_app_root(window);
                            cx.notify();
                        }))
                        .child(
                            Icon::new(IconName::Settings)
                                .xsmall()
                                .text_color(token_hsla(t.text_dim)),
                        ),
                ),
        )
        .children(self.render_linux_window_controls(window, t, cx))
        .into_any_element()
    }
}

impl KnotQApp {
    pub(crate) fn render_scheme_color_popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let (anchor, scheme_id) = self.scheme_color_popover?;
        let t = self.theme();
        let current = self
            .workspace
            .schemes
            .iter()
            .find(|(_, scheme)| scheme.id == scheme_id)
            .map(|(_, scheme)| scheme.color_index)
            .unwrap_or_default();
        let card_w = 230.0;
        let card_h = 178.0;
        let viewport_w = px(f32::from(window.viewport_size().width));
        let viewport_h = px(f32::from(window.viewport_size().height));
        let left = clamped_popover_left(anchor.x - px(18.0), px(card_w), viewport_w);
        let top = popover_top_biased_below(anchor.y + px(8.0), px(card_h), viewport_h);
        let scrim = div()
            .id("scheme-color-popover-scrim")
            .absolute()
            .inset_0()
            .bg(token_rgba(0x00000001))
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.scheme_color_popover = None;
                cx.notify();
            }));
        let mut colors = div().grid().grid_cols(6).gap(px(7.0));
        for index in 0..PALETTE.len() {
            let index = index as u8;
            colors = colors.child(
                div()
                    .id(("scheme-color-option", index as usize))
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded(px(6.0))
                    .bg(palette_hsla(scheme_color(index, t.is_dark), 1.0))
                    .border_2()
                    .border_color(token_rgba(if current == index { t.text_primary } else { 0x00000000 }))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.apply(Command::SetSchemeColor { id: scheme_id, color_index: index }, cx);
                        this.scheme_color_popover = None;
                    })),
            );
        }
        Some(
            div()
                .id("scheme-color-popover")
                .absolute()
                .inset_0()
                .child(scrim)
                .child(
                    div()
                        .id("scheme-color-popover-card")
                        .absolute()
                        .left(left)
                        .top(top)
                        .w(px(card_w))
                        .bg(token_hsla(t.bg_modal))
                        .border_1()
                        .border_color(token_rgba(t.border_overlay))
                        .rounded(px(10.0))
                        .shadow_lg()
                        .p(px(14.0))
                        .child(colors)
                        .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation()),
                )
                .into_any_element(),
        )
    }
}
