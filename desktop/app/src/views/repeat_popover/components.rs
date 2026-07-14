use super::*;

pub(super) fn rp_row(
    id: &'static str,
    label: &'static str,
    selected: bool,
    t: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    div()
        .id(id)
        .h(px(25.0))
        .px(px(9.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .cursor_pointer()
        .font_family(crate::theme_gpui::FONT_UI)
        .text_size(px(11.0))
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
        .on_click(on_click)
        .child(
            div()
                .w(px(14.0))
                .flex_shrink_0()
                .text_color(token_hsla(t.text_primary))
                .child(if selected { "✓" } else { "" }),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label),
        )
        .into_any_element()
}

pub(super) fn rp_selector_row(
    id: &'static str,
    label: &'static str,
    value: String,
    _open: bool,
    t: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    let value_chip = div()
        .id(id)
        .ml(px(-6.0))
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(token_hsla(t.text_primary))
        .line_height(px(15.0))
        .hover({
            let hover = t.row_hover;
            move |s| s.bg(token_rgba(hover))
        })
        .on_click(on_click)
        .child(
            div()
                .max_w(px(136.0))
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(value),
        );

    div()
        .flex()
        .gap(px(8.0))
        .px(px(14.0))
        .h(px(25.0))
        .items_center()
        .font_family(crate::theme_gpui::FONT_UI)
        .text_size(px(11.0))
        .child(
            div()
                .w(px(112.0))
                .flex_shrink_0()
                .text_color(token_hsla(t.text_dim))
                .whitespace_nowrap()
                .child(label),
        )
        .child(div().min_w_0().flex().items_center().child(value_chip))
        .into_any_element()
}

pub(super) struct RpRepeatTypeMenuOptions {
    pub(super) repeat_exists: bool,
    pub(super) complex_repeat: bool,
    pub(super) active_mode: Option<RepeatMode>,
    pub(super) target: RepeatTarget,
    pub(super) left: gpui::Pixels,
    pub(super) top: gpui::Pixels,
    pub(super) t: Theme,
}

pub(super) fn rp_repeat_type_menu(
    options: RpRepeatTypeMenuOptions,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let RpRepeatTypeMenuOptions {
        repeat_exists,
        complex_repeat,
        active_mode,
        target,
        left,
        top,
        t,
    } = options;
    div()
        .id("rp-type-menu")
        .absolute()
        .left(left)
        .top(top)
        .w(px(REPEAT_POPOVER_WIDTH))
        .rounded(px(6.0))
        .border_1()
        .border_color(token_rgba(t.border_overlay))
        .bg(token_hsla(t.bg_modal))
        .shadow_lg()
        .occlude()
        .overflow_hidden()
        .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
        .child(rp_row(
            "rp-none",
            knotq_l10n::t("repeat.type.none"),
            !repeat_exists,
            t,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.clear_repeat_for_target(target, cx);
                cx.stop_propagation();
            }),
        ))
        .child(rp_row(
            "rp-daily",
            RepeatMode::Daily.label(),
            active_mode == Some(RepeatMode::Daily),
            t,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.set_repeat_mode(target, RepeatMode::Daily, cx);
                cx.stop_propagation();
            }),
        ))
        .child(rp_row(
            "rp-weekly",
            RepeatMode::Weekly.label(),
            active_mode == Some(RepeatMode::Weekly),
            t,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.set_repeat_mode(target, RepeatMode::Weekly, cx);
                cx.stop_propagation();
            }),
        ))
        .child(rp_row(
            "rp-monthly",
            RepeatMode::Monthly.label(),
            active_mode == Some(RepeatMode::Monthly),
            t,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.set_repeat_mode(target, RepeatMode::Monthly, cx);
                cx.stop_propagation();
            }),
        ))
        .child(rp_row(
            "rp-yearly",
            RepeatMode::Yearly.label(),
            active_mode == Some(RepeatMode::Yearly),
            t,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.set_repeat_mode(target, RepeatMode::Yearly, cx);
                cx.stop_propagation();
            }),
        ))
        .when(complex_repeat, |menu| {
            menu.child(rp_row(
                "rp-custom",
                knotq_l10n::t("repeat.type.custom"),
                true,
                t,
                cx.listener(|_this, _: &ClickEvent, _w, cx| cx.stop_propagation()),
            ))
        })
        .into_any_element()
}

pub(super) fn repeat_type_menu_height(complex_repeat: bool) -> f32 {
    5.0 * 25.0 + if complex_repeat { 25.0 } else { 0.0 }
}

pub(super) fn rp_weekday_chip(
    weekday: RepeatWeekday,
    active: bool,
    target: RepeatTarget,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    repeat_weekday_button(weekday, active, target, t, cx)
}

pub(super) fn rp_repeat_end_row(
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
        .map(|date| {
            format!(
                "{} {}, {}",
                knotq_date_util::month_short_name(date.month()),
                date.day(),
                date.year()
            )
        })
        .unwrap_or_else(|| knotq_l10n::t("repeat.value.never").to_string());
    let default_until = until_date.unwrap_or_else(|| {
        event_datetime
            .map(|dt| dt.with_timezone(&Local).date_naive())
            .unwrap_or_else(|| Local::now().date_naive())
    });
    let display_month = NaiveDate::from_ymd_opt(default_until.year(), default_until.month(), 1)
        .unwrap_or(default_until);

    let value_button = div()
        .id("rp-end-select")
        .h(px(22.0))
        .ml(px(-6.0))
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .font_family(crate::theme_gpui::FONT_UI)
        .text_size(px(11.0))
        .text_color(token_hsla(t.text_primary))
        .hover({
            let hover = t.button_hover;
            move |s| s.bg(token_rgba(hover))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            if let Some(popup) = this.repeat_popover.as_mut() {
                popup.until_open = !popup.until_open;
                popup.type_menu_open = false;
                popup.end_menu_open = false;
                if popup.until_open {
                    popup.until_display_month = Some(display_month);
                }
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .child(value_label);

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(14.0))
        .h(px(25.0))
        .font_family(crate::theme_gpui::FONT_UI)
        .text_size(px(11.0))
        .child(
            div()
                .w(px(112.0))
                .flex_shrink_0()
                .text_color(token_hsla(t.text_dim))
                .whitespace_nowrap()
                .child(knotq_l10n::t("repeat.field.end")),
        )
        .child(div().flex().items_center().min_w_0().child(value_button))
        .into_any_element()
}

pub(super) fn repeat_popover_estimated_height(
    shows_scope: bool,
    has_simple_repeat: bool,
    weekly: bool,
) -> f32 {
    let mut height = 25.0;
    if shows_scope {
        height += 2.0 * 25.0;
    }
    if has_simple_repeat && weekly {
        height += 25.0;
    }
    if has_simple_repeat {
        height += 25.0;
    }
    height + 12.0
}

pub(super) fn rp_clear_button(
    id: &'static str,
    t: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    div()
        .id(id)
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
        .hover({
            let hover = t.row_hover;
            move |s| s.bg(token_rgba(hover))
        })
        .child("-")
        .on_click(on_click)
        .into_any_element()
}

pub(super) fn repeat_weekday_button(
    weekday: RepeatWeekday,
    active: bool,
    target: RepeatTarget,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let day_index = weekday.num_days_from_monday() as usize;
    div()
        .id(("repeat-weekday", day_index))
        .h(px(18.0))
        .w(px(18.0))
        .rounded(px(99.0))
        .border_1()
        .border_color(token_rgba(if active {
            t.caret_color
        } else {
            t.divider_faint
        }))
        .bg(token_rgba(if active { t.row_selected } else { 0x00000000 }))
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(token_hsla(if active {
            t.text_primary
        } else {
            t.text_soft
        }))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover({
            let c = t.row_hover;
            move |s| s.bg(token_rgba(c))
        })
        .child(repeat_weekday_initial(weekday))
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.set_weekday_for_target(target, weekday, cx);
            cx.stop_propagation();
        }))
        .into_any_element()
}

pub(super) use knotq_rrule::weekday_util::repeat_weekday_initial;
