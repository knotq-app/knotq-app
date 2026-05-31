use super::*;

impl KnotQApp {
    pub(super) fn event_popup_base_repeat_item(
        &self,
        scheme_id: SchemeId,
        item_id: ItemId,
    ) -> Option<Item> {
        let popup = self.event_popup.as_ref()?;
        if popup.scheme_id != scheme_id || popup.item_id != item_id {
            return None;
        }
        let mut item = self
            .workspace
            .scheme(scheme_id)
            .and_then(|scheme| scheme.item(item_id))
            .cloned()?;
        item.repeats = popup.draft_repeats.clone();
        Some(item)
    }

    pub(super) fn set_event_repeat_mode(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        mode: Option<EventRepeatMode>,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(scheme_id) {
            return;
        }
        let repeats = match mode {
            None => None,
            Some(mode) => {
                let Some(item) = self.event_popup_base_repeat_item(scheme_id, item_id) else {
                    return;
                };
                Some(event_repeat_for_mode(&item, mode))
            }
        };
        let Some(popup) = self.event_popup.as_mut() else {
            return;
        };
        if popup.scheme_id != scheme_id || popup.item_id != item_id {
            return;
        }
        popup.draft_repeats = repeats;
        popup.repeats_dirty = true;
        popup.repeat_menu_open = false;
        popup.until_picker_open = false;
        cx.notify();
    }

    pub(super) fn set_event_repeat_end(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        end: RepeatEnd,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(scheme_id) {
            return;
        }
        let repeats = self
            .event_popup
            .as_ref()
            .filter(|popup| popup.scheme_id == scheme_id && popup.item_id == item_id)
            .and_then(|popup| popup.draft_repeats.as_ref())
            .and_then(|repeat| repeat_with_end(repeat, end));
        if let Some(repeats) = repeats {
            let Some(popup) = self.event_popup.as_mut() else {
                return;
            };
            popup.draft_repeats = Some(repeats);
            popup.repeats_dirty = true;
            popup.repeat_menu_open = false;
            popup.until_picker_open = false;
            cx.notify();
        }
    }

    pub(super) fn toggle_event_repeat_weekday(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        weekday: RepeatWeekday,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(scheme_id) {
            return;
        }
        let Some(item) = self.event_popup_base_repeat_item(scheme_id, item_id) else {
            return;
        };
        let (interval, end, mut weekdays) =
            match item.repeats.as_ref().and_then(editable_simple_recurrence) {
                Some(SimpleRecurrence::Weekly {
                    interval,
                    weekdays,
                    end,
                }) => (interval, end, weekdays),
                _ => (1, RepeatEnd::Never, vec![default_repeat_weekday(&item)]),
            };
        if let Some(position) = weekdays.iter().position(|day| *day == weekday) {
            if weekdays.len() > 1 {
                weekdays.remove(position);
            }
        } else {
            weekdays.push(weekday);
        }
        weekdays.sort_unstable_by_key(|day| repeat_weekday_index(*day));
        weekdays.dedup();
        let Some(popup) = self.event_popup.as_mut() else {
            return;
        };
        if popup.scheme_id != scheme_id || popup.item_id != item_id {
            return;
        }
        popup.draft_repeats = Some(recurrence_with_simple(
            item.repeats.as_ref(),
            SimpleRecurrence::Weekly {
                interval,
                weekdays,
                end,
            },
        ));
        popup.repeats_dirty = true;
        popup.repeat_menu_open = false;
        popup.until_picker_open = false;
        cx.notify();
    }

    pub(super) fn set_event_popup_notification_offset(
        &mut self,
        offset_secs: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let read_only = self
            .event_popup
            .as_ref()
            .is_some_and(|popup| self.workspace.is_scheme_read_only(popup.scheme_id));
        if read_only {
            return;
        }
        let Some(popup) = self.event_popup.as_mut() else {
            return;
        };
        popup.draft_notification_offset_secs = offset_secs;
        popup.notification_dirty = true;
        popup.notification_menu_open = false;
        popup.until_picker_open = false;
        cx.notify();
    }

    pub(super) fn cancel_event_scope_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(popup) = self.event_popup.as_mut() {
            if popup.scope_dialog_only {
                self.event_popup = None;
                self.event_popup_title_subscription = None;
                cx.notify();
                return;
            }
            popup.scope_action = None;
            cx.notify();
        }
    }

    pub(super) fn apply_event_scope_choice(&mut self, scope: RepeatScope, cx: &mut Context<Self>) {
        let action = self
            .event_popup
            .as_ref()
            .and_then(|popup| popup.scope_action);
        match action {
            Some(EventScopeAction::Delete) => self.delete_event_popup_item_or_occurrence(scope, cx),
            _ => self.commit_event_popup_with_scope(scope, cx),
        }
    }

    pub(super) fn delete_event_popup_item_or_occurrence(
        &mut self,
        scope: RepeatScope,
        cx: &mut Context<Self>,
    ) {
        let Some(popup) = self.event_popup.take() else {
            return;
        };
        self.event_popup_title_subscription = None;
        self.close_date_popover();

        if popup.created_from_calendar {
            self.delete_created_calendar_popup_item(popup, cx);
            cx.notify();
            return;
        }

        let Some(scheme) = self.workspace.scheme(popup.scheme_id) else {
            return;
        };
        if scheme.is_read_only() {
            return;
        }
        let Some(item) = scheme.item(popup.item_id).cloned() else {
            return;
        };

        let mut delete_item = item.clone();
        if popup.draft_repeats.is_some() {
            delete_item.repeats = popup.draft_repeats.clone();
        }

        let Some(command) = event_popup_delete_command(
            &delete_item,
            popup.scheme_id,
            popup.item_id,
            popup.occurrence,
            popup.occurrence_index,
            event_delete_scope(scope),
        ) else {
            return;
        };

        self.apply(command, cx);
        cx.notify();
    }
}

fn event_delete_scope(scope: RepeatScope) -> EventDeleteScope {
    match scope {
        RepeatScope::ThisEvent => EventDeleteScope::ThisEvent,
        RepeatScope::AllFuture => EventDeleteScope::AllFuture,
        RepeatScope::AllEvents => EventDeleteScope::AllEvents,
    }
}
