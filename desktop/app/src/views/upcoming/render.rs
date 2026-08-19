use super::*;

impl KnotQApp {
    pub fn render_upcoming(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let rows = self.upcoming_rows(cx);
        let UpcomingRows {
            assignments,
            reminders,
            upcoming,
        } = rows;

        let scroll_content = div()
            .id("upcoming-scroll")
            .flex_1()
            .w_full()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(px(8.0))
            .px(px(4.0))
            .child(self.render_section(
                knotq_l10n::t("upcoming.section.assignments"),
                &assignments,
                knotq_l10n::t("upcoming.empty.none"),
                "asgn",
                cx,
            ))
            .child(self.render_section(
                knotq_l10n::t("upcoming.section.reminders"),
                &reminders,
                knotq_l10n::t("upcoming.empty.none"),
                "rem",
                cx,
            ))
            .child(self.render_section(
                knotq_l10n::t("upcoming.section.upcoming"),
                &upcoming,
                knotq_l10n::t("upcoming.empty.none_today"),
                "up",
                cx,
            ));
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
                                            OpenEventPopupArgs {
                                                scheme_id,
                                                item_id,
                                                occurrence: occurrence_for_popup.clone(),
                                                occurrence_index,
                                                start,
                                                end,
                                                anchor: event.position,
                                                select_title: false,
                                                created_from_calendar: false,
                                            },
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    })
                                })
                                .on_click(cx.listener(
                                    move |this, _event: &ClickEvent, _window, cx| {
                                        // Completion is local state, so even
                                        // read-only imported events can be toggled.
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
