use super::*;
use chrono::Weekday;
pub(super) use knotq_date_util::{
    days_in_month, month_start, next_month, prev_month, weekday_name_initial,
};

const UNTIL_CALENDAR_WEEKDAY_ORDER: [Weekday; 7] = [
    Weekday::Sun,
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
];

pub(super) fn until_mini_calendar_popup(
    display_month: NaiveDate,
    selected: Option<NaiveDate>,
    left: gpui::Pixels,
    top: gpui::Pixels,
    scheme_id: SchemeId,
    item_id: ItemId,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let month_start = NaiveDate::from_ymd_opt(display_month.year(), display_month.month(), 1)
        .unwrap_or(display_month);
    let first_weekday = month_start.weekday().num_days_from_sunday() as usize;
    let num_days = days_in_month(display_month.year(), display_month.month()) as usize;
    let month_label = format!(
        "{} {}",
        knotq_date_util::month_name(display_month.month()),
        display_month.year()
    );
    let selected_day_text = selected_date_text_color(t);

    let day_headers: Vec<gpui::AnyElement> = UNTIL_CALENDAR_WEEKDAY_ORDER
        .iter()
        .map(|d| {
            div()
                .h(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .font_family(FONT_UI)
                .text_color(token_hsla(t.text_dim))
                .child(weekday_name_initial(*d))
                .into_any_element()
        })
        .collect();

    let mut day_cells: Vec<gpui::AnyElement> = Vec::new();
    for cell in 0usize..42 {
        if cell < first_weekday || cell >= first_weekday + num_days {
            day_cells.push(
                div()
                    .id(("ep-until-empty", cell))
                    .h(px(22.0))
                    .into_any_element(),
            );
            continue;
        }
        let day = (cell - first_weekday + 1) as u32;
        let date = NaiveDate::from_ymd_opt(display_month.year(), display_month.month(), day)
            .unwrap_or(month_start);
        let is_selected = selected == Some(date);
        let cell_id = display_month.year() as u32 * 10000 + display_month.month() * 100 + day;
        day_cells.push(
            div()
                .id(("ep-until-day", cell_id))
                .h(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    let end = repeat_end_for_local_date(date);
                    this.set_event_repeat_end(scheme_id, item_id, end, cx);
                    if let Some(p) = this.event_popup.as_mut() {
                        p.until_picker_open = false;
                    }
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(
                    div()
                        .w(px(22.0))
                        .h(px(22.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(99.0))
                        .text_size(px(11.0))
                        .font_family(FONT_UI)
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

    div()
        .id("event-popup-until-calendar")
        .absolute()
        .left(left)
        .top(top)
        .w(px(UNTIL_CALENDAR_WIDTH))
        .bg(token_hsla(t.bg_modal))
        .border_1()
        .border_color(token_rgba(t.border_overlay))
        .rounded(px(6.0))
        .shadow_lg()
        .occlude()
        .px(px(7.0))
        .pt(px(7.0))
        .pb(px(7.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .id("ep-until-prev")
                        .w(px(22.0))
                        .h(px(22.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .font_family(FONT_UI)
                        .text_color(token_hsla(t.text_dim))
                        .hover({
                            let h = t.row_hover;
                            move |s| s.bg(token_rgba(h))
                        })
                        .child("‹")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            if let Some(p) = this.event_popup.as_mut() {
                                let cur = p.until_display_month.unwrap_or(display_month);
                                p.until_display_month = Some(prev_month(cur));
                            }
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.0))
                        .font_family(FONT_UI)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(token_hsla(t.text_primary))
                        .child(month_label),
                )
                .child(
                    div()
                        .id("ep-until-next")
                        .w(px(22.0))
                        .h(px(22.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .font_family(FONT_UI)
                        .text_color(token_hsla(t.text_dim))
                        .hover({
                            let h = t.row_hover;
                            move |s| s.bg(token_rgba(h))
                        })
                        .child("›")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            if let Some(p) = this.event_popup.as_mut() {
                                let cur = p.until_display_month.unwrap_or(display_month);
                                p.until_display_month = Some(next_month(cur));
                            }
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
                .child(destructive_minus_button(
                    "ep-until-clear",
                    t,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_event_repeat_end(scheme_id, item_id, RepeatEnd::Never, cx);
                        if let Some(p) = this.event_popup.as_mut() {
                            p.until_picker_open = false;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )),
        )
        .child(div().grid().grid_cols(7).children(day_headers))
        .child(div().grid().grid_cols(7).gap(px(1.0)).children(day_cells))
        .into_any_element()
}
