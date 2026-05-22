use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, Entity, IntoElement, ScrollWheelEvent, Window};
use gpui_component::input::{InputEvent, InputState};
use knotq_index::search::{
    search_hits as query_search_hits, SearchHit, SearchHitStatus, SearchOptions, SearchTarget,
};

use crate::app::{KnotQApp, View, daily_queue_marker_color, DAILY_QUEUE_TITLE};
use crate::theme_gpui::{
    date_status_color, event_status_color, palette_hsla, scheme_color, token_hsla, token_rgba,
    FONT_MONO, FONT_SIZE_BODY, FONT_SIZE_CAPTION2, FONT_UI,
};

impl KnotQApp {
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_repeat_popover();
        self.cancel_event_popup_without_commit(cx);
        self.search_open = true;
        self.search_selected_index = 0;
        let input = self.ensure_search_input(window, cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_open {
            return;
        }
        self.search_open = false;
        self.search_selected_index = 0;
        self.clear_search_input(window, cx);
        if self.selection.view == View::Scheme {
            self.focus_current_editor(window, cx);
        } else {
            self.focus_app_root(window);
        }
        cx.notify();
    }

    pub(crate) fn ensure_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = self.search_input.clone() {
            return input;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search KnotQ"));
        let sub = cx.subscribe_in(&input, window, Self::on_search_input_event);
        self.search_input = Some(input.clone());
        self._search_subscription = Some(sub);
        input
    }

    fn clear_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.search_input.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn on_search_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.search_selected_index = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                let query = input.read(cx).value().to_string();
                let hits = self.search_hits(&query);
                if let Some(hit) = hits
                    .get(self.search_selected_index.min(hits.len().saturating_sub(1)))
                    .cloned()
                {
                    self.open_search_hit(hit, window, cx);
                }
            }
            InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    pub fn select_next_search_result(&mut self, cx: &mut Context<Self>) {
        if !self.search_open {
            cx.propagate();
            return;
        }
        let len = self.current_search_hits(cx).len();
        if len > 0 {
            self.search_selected_index = (self.search_selected_index + 1) % len;
            cx.notify();
        }
    }

    pub fn select_previous_search_result(&mut self, cx: &mut Context<Self>) {
        if !self.search_open {
            cx.propagate();
            return;
        }
        let len = self.current_search_hits(cx).len();
        if len > 0 {
            self.search_selected_index = if self.search_selected_index == 0 {
                len - 1
            } else {
                self.search_selected_index - 1
            };
            cx.notify();
        }
    }

    pub fn render_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme();
        let input = self.ensure_search_input(window, cx);
        let query = input.read(cx).value().to_string();
        let hits = self.search_hits(&query);
        if self.search_selected_index >= hits.len() {
            self.search_selected_index = hits.len().saturating_sub(1);
        }

        let mut result_rows = Vec::new();
        for (idx, hit) in hits.iter().enumerate() {
            let hit_for_click = hit.clone();
            let selected = idx == self.search_selected_index;
            let is_list_hit = matches!(
                &hit.target,
                SearchTarget::Calendar
                    | SearchTarget::DailyQueue { item_id: None, .. }
                    | SearchTarget::Scheme { item_id: None, .. }
            );
            let label_text = if is_list_hit {
                hit.title.clone()
            } else {
                hit.scheme_name.clone()
            };
            let show_detail = !is_list_hit && !hit.detail.trim().is_empty();
            let show_title = !is_list_hit && !hit.title.trim().is_empty();
            let title_text = hit.title.clone();
            let detail_text = hit.detail.clone();
            let detail_color = search_hit_detail_color(hit, token_hsla(t.text_highlight), t.is_dark)
                .unwrap_or_else(|| token_hsla(t.text_soft));
            let color = hit
                .color_override
                .map(token_hsla)
                .or_else(|| {
                    hit.color_index
                        .map(|color_index| palette_hsla(scheme_color(color_index, t.is_dark), 1.0))
                })
                .unwrap_or_else(|| {
                    token_hsla(if t.is_dark {
                        0xffffffff
                    } else {
                        t.text_primary
                    })
                });
            let row_bg = if selected {
                token_rgba(t.row_hover_strong)
            } else if idx % 2 == 1 {
                token_rgba(if t.is_dark { 0xffffff08 } else { 0x00000006 })
            } else {
                token_rgba(0x00000000)
            };
            result_rows.push(
                div()
                    .id(("search-row", idx))
                    .flex()
                    .content_stretch()
                    .mx(px(6.0))
                    .my(px(1.0))
                    .h(px(38.0))
                    .flex_shrink_0()
                    .rounded(px(3.0))
                    .bg(row_bg)
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover({
                        let h = t.row_hover;
                        move |s| s.bg(token_rgba(h))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.open_search_hit(hit_for_click.clone(), window, cx);
                    }))
                    .child(
                        div()
                            .w(px(1.5))
                            .flex_shrink_0()
                            .bg(color)
                            .ml(px(4.0))
                            .mr(px(5.0))
                            .my(px(6.0))
                            .rounded(px(1.0)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pr(px(6.0))
                            .py(px(5.0))
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .when(is_list_hit, |s| s.justify_center())
                            .child(
                                div()
                                    .flex()
                                    .w_full()
                                    .min_w_0()
                                    .justify_between()
                                    .items_baseline()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(FONT_SIZE_CAPTION2))
                                            .line_height(px(12.0))
                                            .font_family(FONT_UI)
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(color)
                                            .child(label_text),
                                    )
                                    .when(show_detail, move |s| {
                                        s.child(
                                            div()
                                                .max_w(px(110.0))
                                                .flex_shrink_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_size(px(FONT_SIZE_CAPTION2 - 1.0))
                                                .line_height(px(11.0))
                                                .font_family(FONT_MONO)
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .text_color(detail_color)
                                                .child(detail_text),
                                        )
                                    }),
                            )
                            .when(show_title, move |s| {
                                s.child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(FONT_SIZE_BODY))
                                        .line_height(px(15.0))
                                        .font_family(FONT_UI)
                                        .font_weight(gpui::FontWeight::NORMAL)
                                        .text_color(token_hsla(t.text_highlight))
                                        .child(title_text),
                                )
                            }),
                    )
                    .into_any_element(),
            );
        }

        div()
            .id("search-dropdown-layer")
            .absolute()
            .top(px(38.0))
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .on_scroll_wheel(|_: &ScrollWheelEvent, _window, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.close_search(window, cx);
            }))
            .child(
                div()
                    .id("search-dropdown")
                    .absolute()
                    .top_0()
                    .right(px(0.0))
                    .w(px(340.0))
                    .max_h(px(460.0))
                    .bg(token_hsla(t.bg_modal))
                    .border_1()
                    .border_color(token_rgba(t.border_overlay))
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .shadow_lg()
                    .occlude()
                    .flex()
                    .flex_col()
                    .on_click(|_: &ClickEvent, _w, cx| cx.stop_propagation())
                    .on_scroll_wheel(|_: &ScrollWheelEvent, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .id("search-results")
                            .flex_1()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .py(px(5.0))
                            .when(result_rows.is_empty(), |s| {
                                s.child(
                                    div()
                                        .px(px(16.0))
                                        .py(px(18.0))
                                        .text_size(px(12.0))
                                        .text_color(token_hsla(t.text_soft))
                                        .child("No results"),
                                )
                            })
                            .when(!result_rows.is_empty(), |s| s.children(result_rows)),
                    ),
            )
            .into_any_element()
    }

    fn search_hits(&self, query: &str) -> Vec<SearchHit> {
        query_search_hits(
            &self.workspace,
            self.time_format,
            query,
            SearchOptions {
                daily_queue_title: DAILY_QUEUE_TITLE,
                daily_queue_marker_color: daily_queue_marker_color(self.theme().is_dark),
            },
        )
    }

    fn current_search_hits(&self, cx: &mut Context<Self>) -> Vec<SearchHit> {
        let query = self
            .search_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        self.search_hits(&query)
    }

    fn open_search_hit(&mut self, hit: SearchHit, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        self.clear_search_input(window, cx);
        match hit.target {
            SearchTarget::Calendar => {
                self.open_union();
                self.focus_app_root(window);
            }
            SearchTarget::DailyQueue { scheme_id, item_id } => {
                self.open_daily_queue(cx);
                if let Some(scheme_id) = scheme_id {
                    self.selection.scheme_id = Some(scheme_id);
                    self.selection.focused_item_id = item_id;
                }
                self.focus_current_editor(window, cx);
            }
            SearchTarget::Scheme { scheme_id, item_id } => {
                self.open_scheme(scheme_id, item_id);
                self.focus_current_editor(window, cx);
            }
        }
        cx.notify();
    }
}

fn search_hit_detail_color(hit: &SearchHit, default: gpui::Hsla, is_dark: bool) -> Option<gpui::Hsla> {
    match hit.status {
        SearchHitStatus::Event { start, end } => Some(event_status_color(
            start.with_timezone(&chrono::Local),
            end.map(|end| end.with_timezone(&chrono::Local)),
            default,
        )),
        SearchHitStatus::Date { dt } => {
            Some(date_status_color(dt.with_timezone(&chrono::Local), default))
        }
        SearchHitStatus::DailyQueue => Some(token_hsla(daily_queue_marker_color(is_dark))),
        SearchHitStatus::None => None,
    }
}
