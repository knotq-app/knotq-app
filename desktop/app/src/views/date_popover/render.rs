use super::*;
use chrono::Weekday;

const DATE_POPOVER_WEEKDAY_ORDER: [Weekday; 7] = [
    Weekday::Sun,
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
];

impl KnotQApp {
    pub fn render_date_popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let popup = self.date_popover.as_ref()?;
        let target = DateTarget {
            scheme_id: popup.scheme_id,
            item_id: popup.item_id,
            kind: popup.kind,
        };
        let year_input = popup.year_input.clone();
        let month_input = popup.month_input.clone();
        let day_input = popup.day_input.clone();
        let hour_input = popup.hour_input.clone();
        let minute_input = popup.minute_input.clone();
        let anchor = popup.anchor;
        let t = self.theme();
        let time_format = self.time_format;
        let uses_meridiem = time_format == TimeFormat::TwelveHour;
        let hour_is_pm = popup.hour_is_pm;
        let selected_day_text = selected_date_text_color(t);
        let card_width = if uses_meridiem {
            DATE_POPOVER_WIDTH_12H
        } else {
            DATE_POPOVER_WIDTH_24H
        };
        let header_gap = if uses_meridiem { 4.0 } else { 8.0 };
        let label = match target.kind {
            DateKind::Start => knotq_l10n::t("event.field.start"),
            DateKind::End => knotq_l10n::t("event.field.end"),
            DateKind::Available => knotq_l10n::t("event.field.available"),
        };

        let current_utc = self
            .date_for_target(target)
            .unwrap_or_else(rounded_local_now_utc);
        let current_local = current_utc.with_timezone(&Local);
        let month = current_local.date_naive();
        let month_start = NaiveDate::from_ymd_opt(month.year(), month.month(), 1).unwrap_or(month);
        let first_weekday = month_start.weekday().num_days_from_sunday() as usize;
        let days_in_month = days_in_month(month.year(), month.month());
        let day_headers: Vec<gpui::AnyElement> = DATE_POPOVER_WEEKDAY_ORDER
            .iter()
            .map(|d| {
                div()
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(FONT_SIZE_CAPTION2))
                    .font_family(FONT_UI)
                    .text_color(token_hsla(t.text_dim))
                    .child(knotq_date_util::weekday_name_initial(*d))
                    .into_any_element()
            })
            .collect();

        let mut day_cells = Vec::new();
        for cell in 0..42 {
            if cell < first_weekday || cell >= first_weekday + days_in_month as usize {
                day_cells.push(
                    div()
                        .id(("date-day-empty", cell))
                        .h(px(24.0))
                        .into_any_element(),
                );
                continue;
            }

            let day = (cell - first_weekday + 1) as u32;
            let date = NaiveDate::from_ymd_opt(month.year(), month.month(), day).unwrap_or(month);
            let is_selected = date == month;
            let target_for_click = target;
            let selected_time = (current_local.hour(), current_local.minute());
            day_cells.push(
                div()
                    .id(("date-day", day))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        let dt = local_dt_from_parts(date, selected_time.0, selected_time.1);
                        this.set_target_date(target_for_click, dt, _w, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .w(px(24.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(99.0))
                            .text_size(px(FONT_SIZE_BODY))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(token_hsla(if is_selected {
                                selected_day_text
                            } else {
                                t.text_highlight
                            }))
                            .bg(token_rgba(if is_selected {
                                t.caret_color
                            } else {
                                0x00000000
                            }))
                            .hover({
                                let (caret, hover) = (t.caret_color, t.row_hover_strong);
                                move |s| s.bg(token_rgba(if is_selected { caret } else { hover }))
                            })
                            .child(day.to_string()),
                    )
                    .into_any_element(),
            );
        }

        let popup_for_clear = target;
        let viewport_width = px(f32::from(window.viewport_size().width));
        let viewport_height = px(f32::from(window.viewport_size().height));
        let desired_left = if anchor.x == px(0.0) {
            px(420.0)
        } else {
            anchor.x
        };
        let left = clamped_popover_left(desired_left, px(card_width), viewport_width);
        let desired_top = if anchor.y == px(0.0) {
            px(132.0)
        } else {
            anchor.y
        };
        let top = popover_top_biased_below(desired_top, px(DATE_POPOVER_HEIGHT), viewport_height);

        let scrim = div()
            .id("date-popover-scrim")
            .absolute()
            .inset_0()
            .bg(token_rgba(0x00000001))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.close_date_popover();
                this.focus_app_root(window);
                cx.stop_propagation();
                cx.notify();
            }));

        let card = div()
            .id("date-popover-card")
            .absolute()
            .left(left)
            .top(top)
            .w(px(card_width))
            .bg(token_hsla(t.bg_modal))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .rounded(px(4.0))
            .px(px(8.0))
            .py(px(3.0))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                this.focus_current_editor(window, cx);
                cx.stop_propagation();
            }))
            .on_click(|_: &ClickEvent, _w, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(27.0))
                    .gap(px(header_gap))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .w(px(32.0))
                            .text_size(px(FONT_SIZE_BODY))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(token_hsla(t.text_highlight))
                            .child(label),
                    )
                    .child(date_group(
                        vec![
                            popover_field("year-field", &year_input, 34.0, t, cx),
                            component_separator("/", t),
                            popover_field("month-field", &month_input, 19.0, t, cx),
                            component_separator("/", t),
                            popover_field("day-field", &day_input, 19.0, t, cx),
                        ],
                        t,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .text_size(px(FONT_SIZE_BODY))
                            .text_color(token_hsla(t.text_muted))
                            .child(knotq_l10n::t("event.date_popover.at")),
                    )
                    .child(if uses_meridiem {
                        date_time_with_meridiem_group(
                            &hour_input,
                            &minute_input,
                            Some(hour_is_pm),
                            t,
                            cx,
                        )
                    } else {
                        date_time_group(&hour_input, &minute_input, t, cx)
                    })
                    .child({
                        let clear_button = div()
                            .id("date-clear")
                            .flex_shrink_0()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(token_rgba(t.text_today))
                            .text_color(token_hsla(t.text_today))
                            .text_size(px(12.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child("-")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                if !this.set_event_popup_date(popup_for_clear, None) {
                                    this.apply(
                                        Command::SetItemDate {
                                            scheme: popup_for_clear.scheme_id,
                                            item: popup_for_clear.item_id,
                                            kind: popup_for_clear.kind,
                                            date: None,
                                        },
                                        cx,
                                    );
                                }
                                this.close_date_popover();
                                this.focus_app_root(window);
                                cx.stop_propagation();
                                cx.notify();
                            }));
                        clear_button
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(div().grid().grid_cols(7).children(day_headers))
                    .child(div().grid().grid_cols(7).gap(px(3.0)).children(day_cells)),
            );

        let layer = div().absolute().inset_0().child(scrim).child(card);

        Some(
            deferred(layer)
                .with_priority(DATE_POPOVER_PRIORITY)
                .into_any_element(),
        )
    }
}
