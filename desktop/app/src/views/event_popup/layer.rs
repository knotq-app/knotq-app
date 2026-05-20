use super::*;
use crate::app::EventPopup;

#[allow(clippy::too_many_arguments)]
pub(super) fn event_popup_layer(
    card: gpui::AnyElement,
    scrim: gpui::AnyElement,
    scope_dialog_open: bool,
    scope_action: Option<EventScopeAction>,
    date_presence_changed: bool,
    popup: &EventPopup,
    item: &Item,
    notification_menu_open: bool,
    notification_offset: i64,
    notification_menu_left: Pixels,
    notification_menu_top: Pixels,
    repeat_menu_open: bool,
    repeat_menu_left: Pixels,
    repeat_menu_top: Pixels,
    scheme_menu: Option<gpui::AnyElement>,
    until_picker_open: bool,
    until_display_month: NaiveDate,
    until_calendar_anchor_y: Pixels,
    card_left: Pixels,
    viewport_width: Pixels,
    viewport_height: Pixels,
    scheme_id: SchemeId,
    item_id: ItemId,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let layer_base = div().id("event-popup-layer").absolute().inset_0().occlude();

    let layer = if scope_dialog_open {
        let mut layer = layer_base;
        if !popup.scope_dialog_only {
            layer = layer.child(card);
        }
        let mut layer = layer.child(scrim);
        if let Some(action) = scope_action {
            let can_apply_future = match action {
                EventScopeAction::ApplyChanges => !date_presence_changed,
                EventScopeAction::Delete => popup
                    .draft_repeats
                    .as_ref()
                    .or(item.repeats.as_ref())
                    .is_some_and(recurrence_can_delete_future),
            };
            let can_apply_all = true;
            let can_apply_this = match action {
                EventScopeAction::ApplyChanges => !date_presence_changed,
                EventScopeAction::Delete => true,
            };
            layer = layer.child(
                div()
                    .id("event-scope-modal-positioner")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(scope_dialog(
                        can_apply_this,
                        can_apply_future,
                        can_apply_all,
                        t,
                        cx,
                    )),
            );
        }
        layer
    } else {
        layer_base.child(scrim).child(card)
    }
    .when(!scope_dialog_open && notification_menu_open, |this| {
        this.child(notification_menu(
            notification_offset,
            notification_menu_left,
            notification_menu_top,
            t,
            cx,
        ))
    })
    .when(!scope_dialog_open && repeat_menu_open, |this| {
        this.child(repeat_type_menu(
            popup.draft_repeats.as_ref(),
            scheme_id,
            item_id,
            repeat_menu_left,
            repeat_menu_top,
            t,
            cx,
        ))
    })
    .when(!scope_dialog_open, |this| {
        this.when_some(scheme_menu, |this, menu| this.child(menu))
    })
    .when(!scope_dialog_open && until_picker_open, |this| {
        let cal_left = clamped_popup_left(
            card_left + px(EVENT_POPUP_WIDTH - UNTIL_CALENDAR_WIDTH),
            px(UNTIL_CALENDAR_WIDTH),
            viewport_width,
        );
        let cal_top = clamped_popup_top(
            until_calendar_anchor_y + px(DATE_POPOVER_Y_OFFSET),
            px(UNTIL_CALENDAR_HEIGHT),
            viewport_height,
        );
        this.child(until_mini_calendar_popup(
            until_display_month,
            popup
                .draft_repeats
                .as_ref()
                .and_then(|r| match simple_repeat_end(r) {
                    Some(RepeatEnd::Until(until)) => Some(until.with_timezone(&Local).date_naive()),
                    _ => None,
                }),
            cal_left,
            cal_top,
            scheme_id,
            item_id,
            t,
            cx,
        ))
    });

    layer.into_any_element()
}
