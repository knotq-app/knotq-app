use super::*;
use crate::app::EventPopup;

pub(super) fn editable_detail_row(
    id: &'static str,
    label: &'static str,
    value: String,
    t: Theme,
    editable: bool,
    on_click: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    let value_chip = if editable {
        div()
            .id(id)
            .ml(px(-6.0))
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .text_color(token_hsla(t.text_primary))
            .line_height(px(15.0))
            .hover({
                let hover = t.row_hover;
                move |s| s.bg(token_rgba(hover))
            })
            .on_click(on_click)
            .child(value)
            .into_any_element()
    } else {
        div()
            .text_color(token_hsla(t.text_primary))
            .line_height(px(15.0))
            .child(value)
            .into_any_element()
    };

    div()
        .flex()
        .items_baseline()
        .gap(px(EVENT_POPUP_DETAIL_GAP))
        .text_size(px(11.0))
        .font_family(FONT_UI)
        .child(
            div()
                .w(px(EVENT_POPUP_DETAIL_LABEL_W))
                .flex_shrink_0()
                .text_color(token_hsla(t.text_dim))
                .whitespace_nowrap()
                .child(label),
        )
        .child(div().min_w_0().flex().items_center().child(value_chip))
        .into_any_element()
}

pub(super) fn delete_event_icon_button(
    has_repeating_occurrence: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    destructive_minus_button(
        "popup-delete-event",
        t,
        cx.listener(move |this, _: &ClickEvent, _window, cx| {
            if has_repeating_occurrence {
                this.close_date_popover();
                if let Some(popup) = this.event_popup.as_mut() {
                    popup.close_all_menus();
                    popup.scope_action = Some(EventScopeAction::Delete);
                }
                cx.notify();
            } else {
                this.delete_event_popup_item_or_occurrence(RepeatScope::AllEvents, cx);
            }
            cx.stop_propagation();
        }),
    )
}

pub(super) fn destructive_minus_button(
    id: &'static str,
    t: Theme,
    on_click: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
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

pub(super) fn notification_menu(
    effective_offset: i64,
    left: gpui::Pixels,
    top: gpui::Pixels,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let options = vec![
        (knotq_l10n::t("event.notification.at_time").to_string(), Some(0)),
        (
            knotq_l10n::t("event.notification.option.5_min_before").to_string(),
            Some(5 * 60),
        ),
        (
            knotq_l10n::t("event.notification.option.10_min_before").to_string(),
            Some(10 * 60),
        ),
        (
            knotq_l10n::t("event.notification.option.30_min_before").to_string(),
            Some(30 * 60),
        ),
        (
            knotq_l10n::t("event.notification.option.1_hour_before").to_string(),
            Some(60 * 60),
        ),
        (
            knotq_l10n::t("event.notification.option.1_day_before").to_string(),
            Some(24 * 60 * 60),
        ),
    ];

    div()
        .id("notification-offset-menu")
        .absolute()
        .top(top)
        .left(left)
        .w(px(176.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(token_rgba(t.border_overlay))
        .bg(token_hsla(t.bg_modal))
        .shadow_lg()
        .occlude()
        .overflow_hidden()
        .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
        .children(
            options
                .into_iter()
                .enumerate()
                .map(|(idx, (label, offset))| {
                    let selected = offset.is_some_and(|offset| offset == effective_offset);
                    div()
                        .id(("notification-offset", idx))
                        .h(px(24.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .text_size(px(11.0))
                        .font_family(FONT_UI)
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
                            this.set_event_popup_notification_offset(offset, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .child(if selected { "✓" } else { "" })
                        .child(label)
                        .into_any_element()
                }),
        )
        .into_any_element()
}

pub(super) fn clamped_popup_left(
    anchor_left: Pixels,
    width: Pixels,
    viewport_width: Pixels,
) -> Pixels {
    clamped_popover_left(anchor_left, width, viewport_width)
}

pub(super) fn clamped_popup_top(
    anchor_top: Pixels,
    height: Pixels,
    viewport_height: Pixels,
) -> Pixels {
    popover_top_biased_below(anchor_top, height, viewport_height)
}

pub(super) fn item_title(text: &str) -> String {
    let title = text.lines().next().unwrap_or("").trim();
    if title.is_empty() {
        knotq_l10n::t("event.item_title.untitled").to_string()
    } else {
        title.to_string()
    }
}

pub(super) fn default_notification_offset(kind: ItemKind, defaults: NotificationDefaults) -> i64 {
    match kind {
        ItemKind::Event => defaults.event_offset_secs,
        ItemKind::Assignment => defaults.assignment_offset_secs,
        ItemKind::Reminder | ItemKind::Procedure => 0,
    }
}

pub(super) fn until_display_month_for_popup(
    popup: &EventPopup,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> NaiveDate {
    let base = popup.until_display_month.unwrap_or_else(|| {
        popup
            .draft_repeats
            .as_ref()
            .and_then(|r| match simple_repeat_end(r) {
                Some(RepeatEnd::Until(until)) => Some(until.with_timezone(&Local).date_naive()),
                _ => None,
            })
            .or_else(|| {
                start
                    .or(end)
                    .map(|dt| dt.with_timezone(&Local).date_naive())
            })
            .unwrap_or_else(|| Local::now().date_naive())
    });
    NaiveDate::from_ymd_opt(base.year(), base.month(), 1).unwrap_or(base)
}
