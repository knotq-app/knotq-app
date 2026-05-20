use super::*;

impl KnotQApp {
    pub(crate) fn open_repeat_popover(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        anchor: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_repeat_popover_with_occurrence_index(
            scheme_id, item_id, anchor, None, window, cx,
        );
    }

    pub(crate) fn open_repeat_popover_with_occurrence_index(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        anchor: gpui::Point<gpui::Pixels>,
        occurrence_index: Option<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(scheme_id) {
            return;
        }
        self.close_date_popover();
        self.repeat_popover = Some(RepeatPopover {
            scheme_id,
            item_id,
            anchor,
            occurrence_index,
            scope: if occurrence_index.is_some() {
                RepeatScope::ThisEvent
            } else {
                RepeatScope::AllEvents
            },
            type_menu_open: false,
            end_menu_open: false,
            until_open: false,
            until_display_month: None,
        });
        cx.notify();
    }

    pub(crate) fn close_repeat_popover(&mut self) {
        self.repeat_popover = None;
        self.recurrence_undo_group = None;
    }

    pub(super) fn clear_repeat_for_target(&mut self, target: RepeatTarget, cx: &mut Context<Self>) {
        if self.workspace.is_scheme_read_only(target.scheme_id) {
            return;
        }
        self.apply(
            Command::SetItemRecurrence {
                scheme: target.scheme_id,
                item: target.item_id,
                repeats: None,
            },
            cx,
        );
        if let Some(popup) = self.repeat_popover.as_mut() {
            popup.type_menu_open = false;
            popup.end_menu_open = false;
            popup.until_open = false;
        }
        cx.notify();
    }

    fn set_repeat_state_for_target(
        &mut self,
        target: RepeatTarget,
        state: RepeatState,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(target.scheme_id) {
            return;
        }
        let Some(item) = self.item_for_repeat_target(target) else {
            return;
        };
        let repeat = repeat_from_state(&state.normalized(item));
        self.apply(
            Command::SetItemRecurrence {
                scheme: target.scheme_id,
                item: target.item_id,
                repeats: Some(repeat),
            },
            cx,
        );
    }

    pub(super) fn set_repeat_mode(
        &mut self,
        target: RepeatTarget,
        mode: RepeatMode,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.item_for_repeat_target(target) else {
            return;
        };

        let mut state = self
            .repeat_for_target(target)
            .as_ref()
            .and_then(|repeat| repeat_state_from_recurrence(repeat, item))
            .unwrap_or_default();

        state.mode = mode;
        self.set_repeat_state_for_target(target, state, cx);
        if let Some(popup) = self.repeat_popover.as_mut() {
            popup.type_menu_open = false;
            popup.end_menu_open = false;
            popup.until_open = false;
        }
        cx.notify();
    }

    pub(super) fn set_repeat_end_for_target(
        &mut self,
        target: RepeatTarget,
        end: RepeatEnd,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.item_for_repeat_target(target) else {
            return;
        };
        let mut state = self
            .repeat_for_target(target)
            .as_ref()
            .and_then(|repeat| repeat_state_from_recurrence(repeat, item))
            .unwrap_or_default();
        state.end = end;
        self.set_repeat_state_for_target(target, state, cx);
        cx.notify();
    }

    pub(super) fn set_weekday_for_target(
        &mut self,
        target: RepeatTarget,
        weekday: RepeatWeekday,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.item_for_repeat_target(target) else {
            return;
        };
        let mut state = self
            .repeat_for_target(target)
            .as_ref()
            .and_then(|repeat| repeat_state_from_recurrence(repeat, item))
            .unwrap_or_default();
        state.mode = RepeatMode::Weekly;
        let Some(position) = state.weekdays.iter().position(|value| *value == weekday) else {
            state.weekdays.push(weekday);
            self.set_repeat_state_for_target(target, state, cx);
            cx.notify();
            return;
        };
        state.weekdays.remove(position);
        self.set_repeat_state_for_target(target, state, cx);
        cx.notify();
    }

    pub(super) fn item_for_repeat_target(&self, target: RepeatTarget) -> Option<&Item> {
        self.workspace
            .scheme(target.scheme_id)
            .and_then(|scheme| scheme.item(target.item_id))
    }

    pub(super) fn repeat_for_target(&self, target: RepeatTarget) -> Option<Recurrence> {
        self.workspace
            .scheme(target.scheme_id)
            .and_then(|scheme| scheme.item(target.item_id))
            .and_then(|item| item.repeats.clone())
    }
}
