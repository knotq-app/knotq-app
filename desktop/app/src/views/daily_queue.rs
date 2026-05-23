use chrono::{Datelike, NaiveDate};
use gpui::prelude::*;
use gpui::{
    div, point, px, relative, ClickEvent, Context, Entity, IntoElement, MouseButton, Pixels,
    ScrollDelta, ScrollWheelEvent, SharedString, Window,
};
use gpui_component::scroll::Scrollbar;

use crate::app::{daily_queue_scheme_is_blank, KnotQApp};
use crate::theme_gpui::{token_hsla, FONT_UI};
use knotq_editor::SchemeEditor;

const DAILY_EDITOR_BOTTOM_PAD: f32 = 4.0;
const DAILY_QUEUE_BOTTOM_SPACER: f32 = 360.0;
const DAILY_SECTION_TOP_PAD: f32 = 4.0;
const DAILY_SECTION_BOTTOM_PAD: f32 = 2.0;
const DAILY_EDITOR_TOP_PAD: f32 = 0.0;
const DAILY_EDITOR_INSET_TOP_PAD: f32 = 0.0;
const DAILY_TITLE_LEFT_PAD: f32 = 33.0;
const DAILY_CARRYOVER_LEFT_PAD: f32 = 36.0;
const DAILY_QUEUE_OLDER_LOAD_THRESHOLD: f32 = 220.0;

impl KnotQApp {
    pub fn render_daily_queue(
        &mut self,
        available_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme();
        let today = self.daily_queue_today;
        let mut dates = self.ensure_daily_queue_window(cx);
        let focused = self
            .selection
            .scheme_id
            .filter(|id| self.workspace.is_daily_queue_scheme(*id));
        if let Some(date) = focused.and_then(|id| self.workspace.daily_queue_date_for_scheme(id)) {
            if date <= today && !dates.contains(&date) {
                dates.push(date);
                dates.sort();
            }
        }
        let focused_item = self.selection.focused_item_id;
        let has_explicit_focus = focused_item.is_some();
        let time_format = self.time_format;
        let theme = self.theme();
        let mut sections = Vec::with_capacity(dates.len());
        let mut seen_today = false;

        for date in dates.drain(..) {
            let Some(editor) = self.ensure_daily_queue_editor(date, window, cx) else {
                continue;
            };
            let Some(scheme_id) = self.workspace.daily_queue_scheme_id(date) else {
                continue;
            };
            let Some(scheme) = self.workspace.scheme(scheme_id).cloned() else {
                continue;
            };
            let show_transfer_yesterday = date == today
                && daily_queue_scheme_is_blank(&scheme)
                && self
                    .workspace
                    .daily_queue_scheme_id(today - chrono::Duration::days(1))
                    .and_then(|scheme_id| self.workspace.scheme(scheme_id))
                    .is_some_and(|scheme| !daily_queue_scheme_is_blank(scheme));
            let already_visible = self.daily_queue_visible_dates.contains(&date);
            if !already_visible && should_skip_daily_queue_day(date, today, &scheme) {
                continue;
            }
            self.daily_queue_visible_dates.insert(date);
            editor.update(cx, |editor, cx| {
                editor.sync_from_scheme(scheme, theme, time_format, window, cx);
                editor.relayout_if_dirty_for_width(px(available_width), window);
                if focused == Some(scheme_id) {
                    if let Some(item_id) = focused_item {
                        editor.focus_item(item_id, window, cx);
                    }
                }
            });
            if has_explicit_focus
                && focused == Some(scheme_id)
                && editor.read(cx).needs_cursor_scroll()
            {
                let editor = editor.clone();
                window.defer(cx, move |_window, cx| {
                    editor.update(cx, |editor, cx| {
                        editor.scroll_to_cursor(cx);
                    });
                });
            }
            if date == today {
                seen_today = true;
            }

            sections.push(
                div()
                    .id(SharedString::from(format!("daily-queue-section-{date}")))
                    .relative()
                    .w_full()
                    .pt(px(DAILY_SECTION_TOP_PAD))
                    .pb(px(DAILY_SECTION_BOTTOM_PAD))
                    .when(date != today, |section| section.opacity(0.78))
                    .child(self.render_daily_queue_day_title(date, today))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .pt(px(DAILY_EDITOR_TOP_PAD))
                            .child(editor)
                            .when(show_transfer_yesterday, |editor_container| {
                                editor_container
                                    .child(self.render_daily_carryover_yesterday_button(cx))
                            }),
                    )
                    .into_any_element(),
            );
        }

        if !self.daily_queue_scroll_initialized {
            if !has_explicit_focus {
                self.selection.scheme_id = self.workspace.daily_queue_scheme_id(today);
                self.selection.focused_item_id = None;
                if seen_today {
                    self.daily_queue_scroll_handle.scroll_to_bottom();
                    self.schedule_daily_queue_scroll_to_bottom(window);
                }
            }
            self.daily_queue_scroll_initialized = true;
        }
        self.restore_daily_queue_scroll_from_bottom_if_needed(window);
        if focused_item.is_some() {
            self.selection.focused_item_id = None;
        }

        let toolbar_scheme_id = focused
            .filter(|scheme_id| {
                self.workspace
                    .daily_queue_date_for_scheme(*scheme_id)
                    .is_some_and(|date| date <= today)
            })
            .or_else(|| self.workspace.daily_queue_scheme_id(today));
        let toolbar = toolbar_scheme_id
            .and_then(|scheme_id| {
                let scheme = self.workspace.scheme(scheme_id).cloned()?;
                self.workspace
                    .daily_queue_date_for_scheme(scheme_id)
                    .and_then(|date| self.daily_queue_editors.get(&date).cloned())
                    .map(|editor| (scheme, editor))
            })
            .map(|(scheme, editor)| self.render_scheme_toolbar(&scheme, editor, cx));

        div()
            .relative()
            .flex_1()
            .h_full()
            .bg(token_hsla(t.bg_app))
            .child(
                div()
                    .id("daily-queue-scroll-shell")
                    .relative()
                    .h_full()
                    .min_h_0()
                    .child(
                        div()
                            .id("daily-queue-scroll")
                            .h_full()
                            .min_h_0()
                            .track_scroll(&self.daily_queue_scroll_handle)
                            .overflow_y_scroll()
                            .on_scroll_wheel(cx.listener(
                                |this, event: &ScrollWheelEvent, _window, cx| {
                                    this.expand_daily_queue_older_if_needed(event, cx);
                                },
                            ))
                            .child(
                                div()
                                    .w_full()
                                    .min_h(relative(1.0))
                                    .flex()
                                    .flex_col()
                                    .justify_end()
                                    .children(sections)
                                    .child(div().h(px(DAILY_QUEUE_BOTTOM_SPACER)).flex_shrink_0()),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .child(
                                Scrollbar::vertical(&self.daily_queue_scroll_handle)
                                    .id("daily-queue-scrollbar"),
                            ),
                    ),
            )
            .when_some(toolbar, |s, toolbar| s.child(toolbar))
            .into_any_element()
    }

    fn expand_daily_queue_older_if_needed(
        &mut self,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        if !scrolls_toward_older_days(event) {
            return;
        }
        if self.daily_queue_scroll_handle.offset().y < -px(DAILY_QUEUE_OLDER_LOAD_THRESHOLD) {
            return;
        }
        if self.expand_daily_queue_older(cx) {
            cx.stop_propagation();
        }
    }

    fn restore_daily_queue_scroll_from_bottom_if_needed(&mut self, window: &mut Window) {
        let Some(distance_from_bottom) = self.daily_queue_preserved_bottom_distance.take() else {
            return;
        };
        let scroll_handle = self.daily_queue_scroll_handle.clone();
        window.on_next_frame(move |window, _cx| {
            let max_y = scroll_handle.max_offset().height;
            let y = (distance_from_bottom - max_y).clamp(-max_y, Pixels::ZERO);
            scroll_handle.set_offset(point(Pixels::ZERO, y));
            window.refresh();
        });
    }

    fn schedule_daily_queue_scroll_to_bottom(&self, window: &mut Window) {
        let scroll_handle = self.daily_queue_scroll_handle.clone();
        window.on_next_frame(move |window, _cx| {
            scroll_handle.scroll_to_bottom();
            window.refresh();

            let scroll_handle = scroll_handle.clone();
            window.on_next_frame(move |window, _cx| {
                scroll_handle.scroll_to_bottom();
                window.refresh();
            });
        });
    }

    pub(crate) fn ensure_daily_queue_editor(
        &mut self,
        date: NaiveDate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SchemeEditor>> {
        if let Some(editor) = self.daily_queue_editors.get(&date).cloned() {
            return Some(editor);
        }
        let scheme_id = self.ensure_daily_queue_scheme_quiet(date, cx);
        let scheme = self.workspace.scheme(scheme_id)?.clone();
        let theme = self.theme();
        let time_format = self.time_format;
        let scroll_handle = self.daily_queue_scroll_handle.clone();
        let editor = cx.new(|cx| {
            let mut editor = SchemeEditor::new(
                scheme_id,
                scheme,
                theme,
                time_format,
                scroll_handle,
                window,
                cx,
            );
            editor.set_top_padding(DAILY_EDITOR_INSET_TOP_PAD, cx);
            editor.set_bottom_padding(DAILY_EDITOR_BOTTOM_PAD, cx);
            editor.suppress_pending_scroll_to_cursor();
            editor
        });
        let sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
        self.daily_queue_editors.insert(date, editor.clone());
        self.daily_queue_editor_subscriptions.insert(date, sub);
        Some(editor)
    }

    fn render_daily_queue_day_title(
        &mut self,
        date: NaiveDate,
        today: NaiveDate,
    ) -> gpui::AnyElement {
        let t = self.theme();
        let delta = (date - today).num_days();
        let label = format!("{} {}, {}", date.format("%B"), date.day(), date.year());
        let is_today = delta == 0;
        let accent = if is_today {
            t.daily_title_active
        } else {
            t.daily_title_muted
        };

        div()
            .flex()
            .items_center()
            .w_full()
            .pl(px(DAILY_TITLE_LEFT_PAD))
            .child(
                div()
                    .text_size(px(24.0))
                    .line_height(px(30.0))
                    .font_family(FONT_UI)
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(token_hsla(accent))
                    .child(label),
            )
            .into_any_element()
    }

    fn render_daily_carryover_yesterday_button(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme();
        let text = t.link;
        let hover_text = t.link_hover;

        div()
            .id("daily-carryover-yesterday-button")
            .absolute()
            .left(px(DAILY_CARRYOVER_LEFT_PAD))
            .top(px(0.0))
            .font_family(FONT_UI)
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_size(px(13.0))
            .line_height(px(18.0))
            .text_color(token_hsla(text))
            .cursor_pointer()
            .hover(move |s| s.text_color(token_hsla(hover_text)))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.carryover_daily_queue(cx);
            }))
            .child("roll over yesterday")
            .into_any_element()
    }
}

fn scrolls_toward_older_days(event: &ScrollWheelEvent) -> bool {
    match event.delta {
        ScrollDelta::Pixels(delta) => delta.y > px(0.0),
        ScrollDelta::Lines(delta) => delta.y > 0.0,
    }
}

fn should_skip_daily_queue_day(
    date: NaiveDate,
    today: NaiveDate,
    scheme: &knotq_model::Scheme,
) -> bool {
    date < today - chrono::Duration::days(1)
        && scheme
            .items
            .iter()
            .all(|item| item.text.trim().is_empty() && item.media.is_empty())
}
