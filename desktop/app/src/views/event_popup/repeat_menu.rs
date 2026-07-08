use super::*;

pub(super) fn repeat_type_menu(
    repeat: Option<&Recurrence>,
    scheme_id: SchemeId,
    item_id: ItemId,
    left: gpui::Pixels,
    top: gpui::Pixels,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let active_mode = repeat.and_then(event_repeat_mode);
    let complex_repeat = repeat.is_some() && active_mode.is_none();

    div()
        .id("repeat-type-menu")
        .absolute()
        .top(top)
        .left(left)
        .w(px(REPEAT_MENU_WIDTH))
        .rounded(px(6.0))
        .border_1()
        .border_color(token_rgba(t.border_overlay))
        .bg(token_hsla(t.bg_modal))
        .shadow_lg()
        .occlude()
        .overflow_hidden()
        .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
        .child(repeat_type_row(
            "repeat-type-none",
            knotq_l10n::t("event.value.none"),
            active_mode.is_none() && !complex_repeat,
            None,
            scheme_id,
            item_id,
            t,
            cx,
        ))
        .child(repeat_type_row(
            "repeat-type-daily",
            EventRepeatMode::Daily.label(),
            active_mode == Some(EventRepeatMode::Daily),
            Some(EventRepeatMode::Daily),
            scheme_id,
            item_id,
            t,
            cx,
        ))
        .child(repeat_type_row(
            "repeat-type-weekly",
            EventRepeatMode::Weekly.label(),
            active_mode == Some(EventRepeatMode::Weekly),
            Some(EventRepeatMode::Weekly),
            scheme_id,
            item_id,
            t,
            cx,
        ))
        .child(repeat_type_row(
            "repeat-type-monthly",
            EventRepeatMode::Monthly.label(),
            active_mode == Some(EventRepeatMode::Monthly),
            Some(EventRepeatMode::Monthly),
            scheme_id,
            item_id,
            t,
            cx,
        ))
        .child(repeat_type_row(
            "repeat-type-yearly",
            EventRepeatMode::Yearly.label(),
            active_mode == Some(EventRepeatMode::Yearly),
            Some(EventRepeatMode::Yearly),
            scheme_id,
            item_id,
            t,
            cx,
        ))
        .into_any_element()
}

pub(super) fn repeat_details_inline_editor(
    repeat: &Recurrence,
    item: &Item,
    scheme_id: SchemeId,
    item_id: ItemId,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let active_mode = event_repeat_mode(repeat);
    let inferred_simple = editable_simple_recurrence(repeat);
    let complex_repeat = inferred_simple.is_none();
    let end = simple_repeat_end(repeat).unwrap_or(RepeatEnd::Never);
    let selected_days = match inferred_simple.as_ref() {
        Some(SimpleRecurrence::Weekly { weekdays, .. }) => {
            if weekdays.is_empty() {
                vec![default_repeat_weekday(item)]
            } else {
                weekdays.clone()
            }
        }
        _ => Vec::new(),
    };

    div()
        .id("popup-repeat-details")
        .mt(px(-2.0))
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .when(complex_repeat, |section| {
            section.child(
                div()
                    .text_size(px(11.0))
                    .font_family(FONT_UI)
                    .text_color(token_hsla(t.text_dim))
                    .child(knotq_l10n::t("event.repeat.custom_rule")),
            )
        })
        .when(active_mode == Some(EventRepeatMode::Weekly), |section| {
            section.child(
                div()
                    .id("popup-repeat-weekdays")
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(EVENT_POPUP_DETAIL_GAP))
                    .child(
                        div()
                            .w(px(EVENT_POPUP_DETAIL_LABEL_W))
                            .flex_shrink_0()
                            .child(""),
                    )
                    .child(div().min_w_0().flex().items_center().gap(px(2.0)).children(
                        repeat_weekdays_for_popup().map(|day| {
                            repeat_weekday_chip(
                                day,
                                selected_days.contains(&day),
                                scheme_id,
                                item_id,
                                t,
                                cx,
                            )
                        }),
                    )),
            )
        })
        .when(!complex_repeat, |section| {
            section.child(repeat_end_inline_editor(
                end,
                item.start.or(item.end),
                t,
                cx,
            ))
        })
        .into_any_element()
}

fn repeat_type_row(
    id: &'static str,
    label: &'static str,
    selected: bool,
    mode: Option<EventRepeatMode>,
    scheme_id: SchemeId,
    item_id: ItemId,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id(id)
        .h(px(25.0))
        .px(px(9.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .font_family(FONT_UI)
        .text_size(px(11.0))
        .cursor_pointer()
        .text_color(token_hsla(if selected {
            t.text_primary
        } else {
            t.text_soft
        }))
        .bg(token_rgba(if selected { t.row_hover } else { 0x00000000 }))
        .hover({
            let hover = t.row_hover;
            move |s| s.bg(token_rgba(hover))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.set_event_repeat_mode(scheme_id, item_id, mode, cx);
            cx.stop_propagation();
        }))
        .child(
            div()
                .w(px(14.0))
                .flex_shrink_0()
                .text_color(token_hsla(t.text_primary))
                .child(if selected { "✓" } else { "" }),
        )
        .child(label)
        .into_any_element()
}

fn repeat_end_inline_editor(
    end: RepeatEnd,
    event_datetime: Option<DateTime<Utc>>,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let until_date = match &end {
        RepeatEnd::Until(until) => Some(until.with_timezone(&Local).date_naive()),
        _ => None,
    };
    let value_label = until_date
        .map(|d| {
            format!(
                "{} {}, {}",
                knotq_date_util::month_short_name(d.month()),
                d.day(),
                d.year()
            )
        })
        .unwrap_or_else(|| knotq_l10n::t("event.repeat.never").to_string());
    let default_until = until_date.unwrap_or_else(|| {
        event_datetime
            .map(|dt| dt.with_timezone(&Local).date_naive())
            .unwrap_or_else(|| Local::now().date_naive())
    });

    // Value button captures the click position so the nested calendar can stay near the row.
    let value_button = div()
        .id("repeat-end-value-button")
        .h(px(22.0))
        .ml(px(-6.0))
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .font_family(FONT_UI)
        .text_size(px(11.0))
        .text_color(token_hsla(t.text_primary))
        .hover({
            let h = t.button_hover;
            move |s| s.bg(token_rgba(h))
        })
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            if this.event_popup.is_some() {
                let anchor_y = event.position().y;
                if let Some(p) = this.event_popup.as_mut() {
                    p.until_picker_open = !p.until_picker_open;
                    p.until_calendar_anchor_y = anchor_y;
                    if p.until_picker_open {
                        p.until_display_month = Some(month_start(default_until));
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .child(value_label);

    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(EVENT_POPUP_DETAIL_GAP))
        .pt(px(1.0))
        .text_size(px(11.0))
        .font_family(FONT_UI)
        .child(
            div()
                .w(px(EVENT_POPUP_DETAIL_LABEL_W))
                .flex_shrink_0()
                .text_color(token_hsla(t.text_dim))
                .child(knotq_l10n::t("event.repeat.end_label")),
        )
        .child(div().flex().items_center().min_w_0().child(value_button))
        .into_any_element()
}

fn repeat_weekday_chip(
    weekday: RepeatWeekday,
    selected: bool,
    scheme_id: SchemeId,
    item_id: ItemId,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id(("popup-repeat-weekday", repeat_weekday_index(weekday)))
        .w(px(18.0))
        .h(px(18.0))
        .rounded(px(99.0))
        .border_1()
        .border_color(token_rgba(if selected {
            t.caret_color
        } else {
            t.divider_faint
        }))
        .bg(token_rgba(if selected {
            t.row_selected
        } else {
            0x00000000
        }))
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_size(px(10.0))
        .text_color(token_hsla(if selected {
            t.text_primary
        } else {
            t.text_soft
        }))
        .cursor_pointer()
        .hover({
            let hover = t.row_hover;
            move |s| s.bg(token_rgba(hover))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.toggle_event_repeat_weekday(scheme_id, item_id, weekday, cx);
            cx.stop_propagation();
        }))
        .child(repeat_weekday_initial(weekday))
        .into_any_element()
}
