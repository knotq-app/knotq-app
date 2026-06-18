use super::*;

impl KnotQApp {
    pub fn render_upcoming(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let now = Utc::now();
        let today_start = Local
            .from_local_datetime(
                &Local::now()
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .unwrap_or_else(|| Local::now().naive_local()),
            )
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(now);
        let horizon = upcoming_range(now).end;
        let today_end = today_start + Duration::days(1);
        self.ensure_daily_queue_calendar_range_loaded(
            Local::now().date_naive(),
            horizon.with_timezone(&Local).date_naive(),
            cx,
        );

        let mut assignments: Vec<UpRow> = Vec::new();
        let mut reminders: Vec<UpRow> = Vec::new();
        let mut upcoming: Vec<UpRow> = Vec::new();

        for scheme in self.workspace.iter_schemes() {
            let is_daily = self.workspace.is_daily_queue_scheme(scheme.id);
            let is_read_only = scheme.is_read_only();
            let scheme_name = self.scheme_display_name(scheme);
            for item in &scheme.items {
                for occ in item.occurrences(today_start, horizon) {
                    if occ.available.is_some_and(|available| available > now) {
                        continue;
                    }
                    let local_when = trigger_time(occ.kind, occ.start, occ.end);
                    let Some(when) = local_when else { continue };
                    let retained_done = occ.state.is_done()
                        && self.retains_completed_calendar_item(scheme.id, item.id, &occ.id);
                    if when < now && occ.state.is_done() && !retained_done {
                        continue;
                    }
                    let when_label = when_label(self.time_format, occ.kind, occ.start, occ.end);
                    let date_color = row_status_color(
                        occ.kind,
                        occ.start,
                        occ.end,
                        token_hsla(t.text_highlight),
                    );
                    let row = UpRow {
                        scheme_id: scheme.id,
                        item_id: item.id,
                        occurrence: occ.id,
                        occurrence_index: occ.occurrence_index,
                        scheme_name: scheme_name.clone(),
                        color_index: scheme.color_index,
                        is_daily,
                        is_read_only,
                        text: item.text(),
                        is_done: occ.state.is_done(),
                        when_label,
                        date_color,
                        sort_key: when,
                        start: occ.start,
                        end: occ.end,
                    };
                    match occ.kind {
                        ItemKind::Assignment => assignments.push(row),
                        ItemKind::Reminder => reminders.push(row),
                        ItemKind::Event if when < today_end => upcoming.push(row),
                        ItemKind::Event => {}
                        ItemKind::Procedure => {}
                    }
                }

                let is_done = item.single_state().is_done();
                let retained_done = is_done
                    && self.retains_completed_calendar_item(
                        scheme.id,
                        item.id,
                        &OccurrenceId::Single,
                    );
                if item.repeats.is_none() && (!is_done || retained_done) {
                    let kind = item.kind();
                    if !matches!(kind, ItemKind::Assignment | ItemKind::Reminder) {
                        continue;
                    }
                    let Some(when) = trigger_time(kind, item.start, item.end) else {
                        continue;
                    };
                    if when >= today_start {
                        continue;
                    }
                    if item.available.is_some_and(|available| available > now) {
                        continue;
                    }
                    let date_color =
                        row_status_color(kind, item.start, item.end, token_hsla(t.text_highlight));
                    let row = UpRow {
                        scheme_id: scheme.id,
                        item_id: item.id,
                        occurrence: OccurrenceId::Single,
                        occurrence_index: 0,
                        scheme_name: scheme_name.clone(),
                        color_index: scheme.color_index,
                        is_daily,
                        is_read_only,
                        text: item.text(),
                        is_done,
                        when_label: when_label(self.time_format, kind, item.start, item.end),
                        date_color,
                        sort_key: when,
                        start: item.start,
                        end: item.end,
                    };
                    match kind {
                        ItemKind::Assignment => assignments.push(row),
                        ItemKind::Reminder => reminders.push(row),
                        _ => {}
                    }
                }
            }
        }
        // Sort by date only — toggling done should not reshuffle the list, since that
        // makes the row "jump" out from under the user's cursor when they click it.
        for v in [&mut assignments, &mut reminders, &mut upcoming] {
            v.sort_by_key(|r| r.sort_key);
        }
        let scroll_content = div()
            .id("upcoming-scroll")
            .flex_1()
            .w_full()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(px(8.0))
            .px(px(4.0))
            .child(self.render_section("Assignments", &assignments, "None", "asgn", cx))
            .child(self.render_section("Reminders", &reminders, "None", "rem", cx))
            .child(self.render_section("Upcoming", &upcoming, "None today", "up", cx));
        let scroll_content = scroll_content.overflow_y_scrollbar().into_any_element();

        div()
            .w(px(258.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(token_hsla(t.bg_app))
            .child(scroll_content)
    }

    fn render_section(
        &mut self,
        heading: &'static str,
        rows: &[UpRow],
        empty_msg: &'static str,
        id_prefix: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.theme();
        let mut heading_color = token_hsla(t.text_primary);
        heading_color.a *= 0.8;
        let mut empty_color = token_hsla(t.text_primary);
        empty_color.a *= 0.5;
        let mut elements: Vec<gpui::AnyElement> = Vec::new();
        if rows.is_empty() {
            elements.push(
                div()
                    .py(px(2.0))
                    .text_size(px(FONT_SIZE_BODY))
                    .line_height(px(16.0))
                    .font_family(crate::theme_gpui::FONT_UI)
                    .text_color(empty_color)
                    .flex()
                    .justify_center()
                    .child(empty_msg)
                    .into_any_element(),
            );
        } else {
            for (i, row) in rows.iter().enumerate() {
                let scheme_id = row.scheme_id;
                let item_id = row.item_id;
                let occurrence = row.occurrence.clone();
                let occurrence_for_popup = row.occurrence.clone();
                let occurrence_index = row.occurrence_index;
                let start = row.start;
                let end = row.end;
                let color = if row.is_daily {
                    token_hsla(daily_queue_marker_color(t.is_dark))
                } else {
                    upcoming_scheme_color(row.color_index, t.is_dark)
                };
                let bg = if i % 2 == 1 {
                    token_rgba(t.row_stripe)
                } else {
                    gpui::Rgba::default()
                };
                let opacity = if row.is_done { 0.35 } else { 1.0 };
                let has_text = !row.text.trim().is_empty();
                let item_text = row.text.clone();
                let when_label = row.when_label.clone();
                let date_color = row.date_color;
                let editable = !row.is_read_only;
                elements.push(
                    div()
                        .w_full()
                        .px(px(0.0))
                        .my(px(0.0))
                        .child(
                            div()
                                .id((id_prefix, i))
                                .relative()
                                .flex()
                                .content_stretch()
                                .w_full()
                                .min_h(px(51.0))
                                .rounded(px(3.0))
                                .bg(bg)
                                .opacity(opacity)
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Right, {
                                    let occurrence_for_popup = occurrence_for_popup.clone();
                                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                        this.focus_app_root(window);
                                        this.open_event_popup(
                                            scheme_id,
                                            item_id,
                                            occurrence_for_popup.clone(),
                                            occurrence_index,
                                            start,
                                            end,
                                            event.position,
                                            false,
                                            false,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    })
                                })
                                .on_click(cx.listener(
                                    move |this, _event: &ClickEvent, _window, cx| {
                                        if !editable {
                                            return;
                                        }
                                        this.toggle_calendar_item(
                                            scheme_id,
                                            item_id,
                                            occurrence.clone(),
                                            cx,
                                        );
                                    },
                                ))
                                .child(
                                    div()
                                        .w(px(1.5))
                                        .flex_shrink_0()
                                        .bg(color)
                                        .ml(px(4.0))
                                        .mr(px(5.0))
                                        .my(px(8.0))
                                        .rounded(px(1.0)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .pl(px(0.0))
                                        .pr(px(8.0))
                                        .pt(px(8.0))
                                        .pb(px(8.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div().relative().w_full().h(px(12.0)).min_w_0().child(
                                                div()
                                                    .absolute()
                                                    .left_0()
                                                    .right(px(72.0))
                                                    .top_0()
                                                    .min_w_0()
                                                    .text_size(px(FONT_SIZE_CAPTION2))
                                                    .line_height(px(12.0))
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .text_color(color)
                                                    .truncate()
                                                    .whitespace_nowrap()
                                                    .overflow_hidden()
                                                    .child(row.scheme_name.clone()),
                                            ),
                                        )
                                        .when(has_text, move |s| {
                                            s.child(
                                                div()
                                                    .text_size(px(FONT_SIZE_BODY))
                                                    .line_height(px(15.0))
                                                    .text_color(token_hsla(t.text_highlight))
                                                    .truncate()
                                                    .whitespace_nowrap()
                                                    .overflow_hidden()
                                                    .child(item_text),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(8.0))
                                        .right(px(8.0))
                                        .child(when_label_element(&when_label, date_color)),
                                ),
                        )
                        .into_any_element(),
                );
            }
        }

        div()
            .flex()
            .flex_col()
            .min_h(px(40.0))
            .py(px(2.0))
            .child(
                div()
                    .py(px(0.0))
                    .px(px(0.0))
                    .text_size(px(FONT_SIZE_BODY))
                    .line_height(px(17.0))
                    .font_family(crate::theme_gpui::FONT_UI)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(heading_color)
                    .flex()
                    .justify_center()
                    .child(heading),
            )
            .children(elements)
    }
}
