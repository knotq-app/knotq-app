use super::*;

impl KnotQApp {
    pub(super) fn render_month_calendar(
        &mut self,
        available_width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme();
        let today = Local::now().date_naive();
        let month_start = self.calendar_month_start();
        let grid_start = month_grid_start(month_start);
        let day_col_w = (available_width / CALENDAR_WEEK_VIEW_DAYS as f32).max(MIN_WEEK_DAY_W);
        let month_grid_w = day_col_w * CALENDAR_WEEK_VIEW_DAYS as f32;
        let start_utc = Local
            .with_ymd_and_hms(
                grid_start.year(),
                grid_start.month(),
                grid_start.day(),
                0,
                0,
                0,
            )
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let end_utc = start_utc + Duration::days(42);
        self.ensure_daily_queue_calendar_range_loaded(
            grid_start,
            grid_start + Duration::days(41),
            cx,
        );

        let all_tasks = self.collect_calendar_tasks(start_utc, end_utc);

        let mut weekday_cells = Vec::new();
        for weekday in MONTH_WEEKDAYS {
            weekday_cells.push(
                div()
                    .w(px(day_col_w))
                    .min_w_0()
                    .flex_none()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_l_1()
                    .border_color(token_rgba(t.border_main))
                    .text_size(px(FONT_SIZE_BODY))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(token_hsla(t.text_dim))
                    .child(weekday_label(weekday))
                    .into_any_element(),
            );
        }

        let month_title_row = div()
            .w(px(month_grid_w))
            .h(px(36.0))
            .bg(token_hsla(t.bg_app))
            .border_b_1()
            .border_color(token_rgba(t.divider_tiny))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_primary))
                    .child(format!("{}", month_start.format("%B %Y"))),
            )
            .child(month_nav(t, cx));

        let header = div()
            .w(px(month_grid_w))
            .h(px(34.0))
            .bg(token_hsla(t.bg_app))
            .border_b_1()
            .border_color(token_rgba(t.divider_tiny))
            .child(
                div()
                    .flex()
                    .w(px(month_grid_w))
                    .h_full()
                    .children(weekday_cells),
            );

        let mut rows = Vec::new();
        for week in 0..6 {
            let mut cells = Vec::new();
            for day in 0..7 {
                let date = grid_start + Duration::days((week * 7 + day) as i64);
                let in_month =
                    date.month() == month_start.month() && date.year() == month_start.year();
                let is_today = date == today;
                let is_past = date < today;
                let mut tasks: Vec<&CalendarTask> = all_tasks
                    .iter()
                    .filter(|task| {
                        task.start
                            .or(task.end)
                            .is_some_and(|dt| dt.date_naive() == date)
                    })
                    .collect();
                tasks.sort_by_key(|task| task.start.or(task.end));

                let mut children = Vec::new();
                children.push(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(20.0))
                        .child(
                            div()
                                .w(px(22.0))
                                .h(px(20.0))
                                .px(px(5.0))
                                .rounded(px(10.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(FONT_SIZE_BODY))
                                .font_weight(if is_today {
                                    gpui::FontWeight::BOLD
                                } else {
                                    gpui::FontWeight::MEDIUM
                                })
                                .text_color(token_hsla(if is_today {
                                    t.text_today
                                } else if in_month {
                                    t.text_primary
                                } else {
                                    t.text_muted
                                }))
                                .child(date.day().to_string()),
                        )
                        .into_any_element(),
                );

                let visible = tasks.iter().take(4);
                for (idx, task) in visible.enumerate() {
                    let item_color = calendar_task_color(task, t.is_dark);
                    let scheme_id = task.scheme_id;
                    let item_id = task.item_id;
                    let occurrence = task.occurrence.clone();
                    let editable = !task.is_read_only;
                    children.push(
                        div()
                            .id(("month-task", week * 100 + day * 10 + idx))
                            .w_full()
                            .min_w_0()
                            .h(px(18.0))
                            .rounded(px(4.0))
                            .px(px(4.0))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .overflow_hidden()
                            .opacity(if task.is_done { 0.78 } else { 1.0 })
                            .when(editable, |s| s.cursor_pointer())
                            .hover({
                                let h = t.row_hover;
                                move |s| s.bg(token_rgba(h))
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                if !editable {
                                    return;
                                }
                                this.toggle_calendar_item(
                                    scheme_id,
                                    item_id,
                                    occurrence.clone(),
                                    cx,
                                );
                            }))
                            .child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .rounded(px(2.0))
                                    .flex_shrink_0()
                                    .bg(item_color),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(FONT_SIZE_CAPTION2))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(token_hsla(t.text_highlight))
                                    .child(self.month_task_label(task)),
                            )
                            .into_any_element(),
                    );
                }
                if tasks.len() > 4 {
                    children.push(
                        div()
                            .h(px(16.0))
                            .px(px(4.0))
                            .text_size(px(10.0))
                            .text_color(token_hsla(t.text_muted))
                            .child(format!("+{} more", tasks.len() - 4))
                            .into_any_element(),
                    );
                }

                cells.push(
                    div()
                        .w(px(day_col_w))
                        .flex_none()
                        .min_w_0()
                        .h_full()
                        .p(px(5.0))
                        .gap(px(2.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .border_l_1()
                        .border_t_1()
                        .border_color(token_rgba(t.divider_faint))
                        .bg(token_rgba(if is_past { t.cal_past } else { t.bg_app }))
                        .opacity(if in_month { 1.0 } else { 0.52 })
                        .children(children)
                        .into_any_element(),
                );
            }

            rows.push(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .w(px(month_grid_w))
                    .children(cells)
                    .into_any_element(),
            );
        }

        div()
            .flex_1()
            .flex()
            .flex_col()
            .h_full()
            .bg(token_hsla(t.bg_app))
            .text_color(token_hsla(t.text_primary))
            .overflow_x_scrollbar()
            .child(
                div()
                    .w(px(month_grid_w))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(month_title_row)
                    .child(header)
                    .child(div().flex_1().min_h_0().flex().flex_col().children(rows)),
            )
            .into_any_element()
    }
    fn month_task_label(&self, task: &CalendarTask) -> String {
        let text = if task.text.trim().is_empty() {
            "(untitled)"
        } else {
            task.text.as_str()
        };
        match task.kind {
            ItemKind::Event => task
                .start
                .map(|dt| format!("{} {}", format_time(self.time_format, dt), text))
                .unwrap_or_else(|| text.to_string()),
            ItemKind::Reminder => task
                .start
                .map(|dt| format!("At {} {}", format_time(self.time_format, dt), text))
                .unwrap_or_else(|| text.to_string()),
            ItemKind::Assignment => task
                .end
                .map(|dt| format!("Due {} {}", format_time(self.time_format, dt), text))
                .unwrap_or_else(|| text.to_string()),
            ItemKind::Procedure => text.to_string(),
        }
    }
}

fn month_nav(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    div()
        .w(px(64.0))
        .h_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .child(
            div()
                .id("month-prev")
                .cursor_pointer()
                .text_size(px(14.0))
                .text_color(token_hsla(t.text_muted))
                .hover({
                    let c = t.text_primary;
                    move |s| s.text_color(token_hsla(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.shift_calendar_period(-1);
                    cx.notify();
                }))
                .child("‹"),
        )
        .child(
            div()
                .id("month-today")
                .cursor_pointer()
                .text_size(px(10.0))
                .text_color(token_hsla(t.text_muted))
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.reset_calendar_period();
                    cx.notify();
                }))
                .child("·"),
        )
        .child(
            div()
                .id("month-next")
                .cursor_pointer()
                .text_size(px(14.0))
                .text_color(token_hsla(t.text_muted))
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.shift_calendar_period(1);
                    cx.notify();
                }))
                .child("›"),
        )
        .into_any_element()
}

fn weekday_label(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

fn month_grid_start(month_start: chrono::NaiveDate) -> chrono::NaiveDate {
    month_start - Duration::days(month_start.weekday().num_days_from_sunday() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn month_calendar_grid_starts_on_sunday() {
        let friday_month_start = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        assert_eq!(
            month_grid_start(friday_month_start),
            NaiveDate::from_ymd_opt(2026, 4, 26).unwrap()
        );

        let sunday_month_start = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap();
        assert_eq!(month_grid_start(sunday_month_start), sunday_month_start);
    }
}
