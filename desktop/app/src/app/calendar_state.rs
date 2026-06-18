use chrono::{DateTime, NaiveDate, Utc};
use gpui::{AppContext, Context, Pixels, Point, Window};
use knotq_commands::{
    event_popup_commit_commands, reset_after_trigger_notification_to_default_command, Command,
    DateEditScope, EventPopupDraft,
};
use knotq_date_util::snapped_calendar_datetime;
use knotq_model::{Item, ItemId, ItemMarker, OccurrenceId, SchemeId};
use knotq_ui::single_line_editor::{SingleLineEditor, SingleLineEditorEvent};

use super::{CalendarOccurrenceKey, EventPopup, EventScopeAction, KnotQApp, RepeatScope};
use knotq_state::mark_past_event_completion_keys_done;

impl KnotQApp {
    pub(crate) fn clear_calendar_pointer_state(&mut self, cx: &mut Context<Self>) -> bool {
        let had_state =
            self.cal_drag.is_some() || self.cal_move.is_some() || self.cal_resize.is_some();
        if had_state {
            self.cal_drag = None;
            self.cal_move = None;
            self.cal_resize = None;
            cx.notify();
        }
        had_state
    }

    pub(crate) fn retains_completed_calendar_item(
        &self,
        scheme_id: SchemeId,
        item_id: ItemId,
        occurrence: &OccurrenceId,
    ) -> bool {
        self.retained_completed_calendar_items
            .contains(&CalendarOccurrenceKey {
                scheme_id,
                item_id,
                occurrence: occurrence.clone(),
            })
    }

    pub(crate) fn sync_retained_completed_calendar_items(
        &mut self,
        keys: &[CalendarOccurrenceKey],
    ) {
        for key in keys.iter().cloned() {
            if self.calendar_occurrence_is_done(&key) {
                self.retained_completed_calendar_items.insert(key);
            } else {
                self.retained_completed_calendar_items.remove(&key);
            }
        }
    }

    fn calendar_occurrence_is_done(&self, key: &CalendarOccurrenceKey) -> bool {
        self.workspace
            .scheme(key.scheme_id)
            .and_then(|scheme| scheme.item(key.item_id))
            .map(|item| item.state_for_occurrence(&key.occurrence))
            .unwrap_or_default()
            .is_done()
    }

    pub fn close_event_popup(&mut self, cx: &mut Context<Self>) {
        self.close_date_popover();
        if let Some(mut popup) = self.event_popup.take() {
            if popup.scope_action.is_some() {
                self.event_popup = Some(popup);
                cx.notify();
                return;
            }
            if self.event_popup_needs_date_scope(&popup) {
                popup.close_all_menus();
                popup.scope_action = Some(EventScopeAction::ApplyChanges);
                self.event_popup = Some(popup);
                cx.notify();
                return;
            }
            self.event_popup_title_subscription = None;
            self.commit_event_popup(popup, RepeatScope::AllEvents, cx);
        }
    }

    pub(crate) fn cancel_event_popup_without_commit(&mut self, cx: &mut Context<Self>) -> bool {
        self.close_date_popover();
        let Some(popup) = self.event_popup.take() else {
            return false;
        };
        self.event_popup_title_subscription = None;
        if popup.created_from_calendar {
            self.delete_created_calendar_popup_item(popup, cx);
        }
        true
    }

    pub(crate) fn delete_created_calendar_popup_item(
        &mut self,
        popup: EventPopup,
        cx: &mut Context<Self>,
    ) {
        let item_exists = self
            .workspace
            .scheme(popup.scheme_id)
            .and_then(|scheme| scheme.item(popup.item_id))
            .is_some();
        if !item_exists {
            self.discard_pending_creation_undo(popup.item_id);
            return;
        }
        if self.workspace.is_scheme_read_only(popup.scheme_id) {
            return;
        }
        let command = Command::DeleteItem {
            scheme: popup.scheme_id,
            item: popup.item_id,
        };
        if self.apply_without_pushing_undo(command, cx).is_some() {
            self.discard_pending_creation_undo(popup.item_id);
        }
    }

    pub(crate) fn commit_event_popup_with_scope(
        &mut self,
        scope: RepeatScope,
        cx: &mut Context<Self>,
    ) {
        self.close_date_popover();
        let Some(popup) = self.event_popup.take() else {
            return;
        };
        self.event_popup_title_subscription = None;
        self.commit_event_popup(popup, scope, cx);
    }

    fn event_popup_needs_date_scope(&self, popup: &EventPopup) -> bool {
        if popup.scope_action.is_some() || popup.occurrence.is_single() {
            return false;
        }
        let Some(scheme) = self.workspace.scheme(popup.scheme_id) else {
            return false;
        };
        if scheme.is_read_only() {
            return false;
        }
        let Some(item) = scheme.item(popup.item_id) else {
            return false;
        };
        item.repeats.is_some()
            && ((popup.start_dirty && item.start != popup.draft_start)
                || (popup.end_dirty && item.end != popup.draft_end))
    }

    fn commit_event_popup(
        &mut self,
        popup: EventPopup,
        date_scope: RepeatScope,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self
            .workspace
            .scheme(popup.scheme_id)
            .and_then(|scheme| scheme.item(popup.item_id))
        else {
            return;
        };
        if self.workspace.is_scheme_read_only(popup.scheme_id) {
            return;
        }

        let mut commands = Vec::new();
        if popup.title_dirty && item.text() != popup.draft_title {
            commands.push(Command::UpdateItemText {
                scheme: popup.scheme_id,
                item: popup.item_id,
                text: popup.draft_title.clone(),
            });
        }

        let draft = EventPopupDraft {
            scheme_id: popup.scheme_id,
            item_id: popup.item_id,
            occurrence: popup.occurrence.clone(),
            occurrence_index: popup.occurrence_index,
            draft_start: popup.draft_start,
            draft_end: popup.draft_end,
            draft_repeats: popup.draft_repeats.clone(),
            draft_notification_offset_secs: popup.draft_notification_offset_secs,
            draft_done: popup.draft_done,
            start_dirty: popup.start_dirty,
            end_dirty: popup.end_dirty,
            repeats_dirty: popup.repeats_dirty,
            notification_dirty: popup.notification_dirty,
            done_dirty: popup.done_dirty,
        };
        commands.extend(event_popup_commit_commands(
            item,
            &draft,
            date_edit_scope(date_scope),
        ));

        if popup.created_from_calendar {
            if let Some(cmd) = Command::from_vec(commands) {
                self.apply_without_pushing_undo(cmd, cx);
            }
            return;
        }

        if let Some(cmd) = Command::from_vec(commands) {
            self.apply(cmd, cx);
        }
    }

    pub(crate) fn open_event_popup(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        occurrence: OccurrenceId,
        occurrence_index: usize,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        anchor: Point<Pixels>,
        select_title: bool,
        created_from_calendar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.event_popup.is_some() {
            self.close_event_popup(cx);
            if self.event_popup.is_some() {
                return;
            }
        } else {
            self.close_date_popover();
        }
        self.close_repeat_popover();
        let Some(scheme) = self.workspace.scheme(scheme_id) else {
            return;
        };
        let read_only = scheme.is_read_only();
        let Some(item) = scheme.item(item_id).cloned() else {
            return;
        };
        let occurrence_state = item.state_for_occurrence(&occurrence);
        let title_input = if read_only {
            self.event_popup_title_subscription = None;
            None
        } else {
            let input = cx.new(|cx| SingleLineEditor::new("Title", item.text(), window, cx));
            let title_subscription =
                cx.subscribe_in(&input, window, Self::on_event_popup_title_input_event);
            if select_title {
                input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
            }
            self.event_popup_title_subscription = Some(title_subscription);
            Some(input)
        };
        let draft_start = if occurrence.is_single() {
            item.start
        } else {
            start.or(item.start)
        };
        let draft_end = if occurrence.is_single() {
            item.end
        } else {
            end.or(item.end)
        };
        let mut popup = EventPopup::new(
            scheme_id,
            item_id,
            &item,
            occurrence,
            &occurrence_state,
            draft_start,
            draft_end,
            anchor,
            occurrence_index,
        );
        popup.title_input = title_input;
        popup.created_from_calendar = created_from_calendar;
        self.event_popup = Some(popup);
        cx.notify();
    }

    fn on_event_popup_title_input_event(
        &mut self,
        input: &gpui::Entity<SingleLineEditor>,
        event: &SingleLineEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let matching_input = self.event_popup.as_ref().is_some_and(|popup| {
            popup
                .title_input
                .as_ref()
                .is_some_and(|title| title == input)
        });
        if !matching_input {
            return;
        }
        let value = input.read(cx).value().to_string();

        match event {
            SingleLineEditorEvent::Change | SingleLineEditorEvent::Submit => {
                self.update_event_popup_title_draft(value, cx);
                if matches!(event, SingleLineEditorEvent::Submit) {
                    self.close_event_popup(cx);
                    self.focus_app_root(window);
                }
            }
            SingleLineEditorEvent::Blur => {
                self.update_event_popup_title_draft(value, cx);
            }
            SingleLineEditorEvent::Cancel => {
                self.cancel_event_popup_without_commit(cx);
                self.focus_app_root(window);
                cx.notify();
            }
            SingleLineEditorEvent::Focus => {}
        }
    }

    fn update_event_popup_title_draft(&mut self, text: String, cx: &mut Context<Self>) {
        let read_only = self
            .event_popup
            .as_ref()
            .is_some_and(|popup| self.workspace.is_scheme_read_only(popup.scheme_id));
        if read_only {
            return;
        }
        if let Some(popup) = self.event_popup.as_mut() {
            if popup.draft_title != text {
                popup.draft_title = text;
                popup.title_dirty = true;
                cx.notify();
            }
        }
    }

    pub fn toggle_calendar_item(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        occurrence: OccurrenceId,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(scheme_id) {
            return;
        }
        if !self.item_allows_occurrence_toggle(scheme_id, item_id, &occurrence) {
            return;
        }
        self.apply(
            Command::ToggleOccurrence {
                scheme: scheme_id,
                item: item_id,
                occurrence,
            },
            cx,
        );
    }

    pub(crate) fn complete_past_event_occurrences(
        &mut self,
        keys: &[CalendarOccurrenceKey],
        now: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> usize {
        let changed = mark_past_event_completion_keys_done(&mut self.workspace, keys, now);
        if changed == 0 {
            return 0;
        }
        // Completion keys can come from any scheme in the background snapshot.
        let all_ids: Vec<_> = self.workspace.schemes.keys().copied().collect();
        for id in all_ids {
            self.dirty_schemes.insert(id);
        }
        self.index_dirty = true;
        self.state.mark_direct_workspace_dirty();
        self.reconcile_workspace_ui_state();
        self.reschedule_notifications();
        cx.notify();
        changed
    }

    pub(crate) fn reschedule_notifications(&mut self) {
        self.service_bus.signal_notifications();
        self.service_bus.signal_timeline();
        self.service_bus.signal_save();
    }

    /// Create a new calendar item from a click or drag on the week view.
    /// - Click → reminder (start date = clicked time)
    /// - Shift+click → assignment (end date = clicked time)
    /// - Drag → event (start + end from drag range)
    /// The item is added to today's daily queue scheme by default, then the
    /// event popup is opened so the user can customize it.
    /// `start_hour`/`end_hour` are hour fractions (0.0–24.0).
    pub(crate) fn create_calendar_item_from_drag(
        &mut self,
        date: NaiveDate,
        start_hour: f32,
        end_hour: f32,
        shift: bool,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_drag = (end_hour - start_hour).abs() > 0.125; // ~7.5 minutes

        let hour_to_datetime = |hour: f32| snapped_calendar_datetime(date, hour);

        let scheme_id = self.ensure_daily_queue_scheme(self.daily_queue_today, cx);

        let mut item = Item::new("");
        item.marker = ItemMarker::Checkbox;

        if is_drag {
            // Event: start + end from drag range.
            let (lo, hi) = if start_hour < end_hour {
                (start_hour, end_hour)
            } else {
                (end_hour, start_hour)
            };
            item.start = Some(hour_to_datetime(lo));
            item.end = Some(hour_to_datetime(hi));
        } else if shift {
            // Assignment: end only.
            item.end = Some(hour_to_datetime(start_hour));
        } else {
            // Reminder: start only.
            item.start = Some(hour_to_datetime(start_hour));
        }

        let item_id = item.id;
        let position = self
            .workspace
            .scheme(scheme_id)
            .map(|s| s.items.len())
            .unwrap_or(0);
        self.apply(
            Command::InsertItem {
                scheme: scheme_id,
                position,
                item,
            },
            cx,
        );

        self.open_event_popup(
            scheme_id,
            item_id,
            OccurrenceId::Single,
            0,
            self.workspace
                .scheme(scheme_id)
                .and_then(|s| s.item(item_id))
                .and_then(|i| i.start),
            self.workspace
                .scheme(scheme_id)
                .and_then(|s| s.item(item_id))
                .and_then(|i| i.end),
            anchor,
            true,
            true,
            window,
            cx,
        );
    }

    /// Move the item from the current popup's scheme to a different scheme.
    pub(crate) fn move_popup_item_to_scheme(
        &mut self,
        target_scheme_id: SchemeId,
        cx: &mut Context<Self>,
    ) {
        let Some((source_scheme_id, item_id, created_from_calendar)) = self
            .event_popup
            .as_ref()
            .map(|popup| (popup.scheme_id, popup.item_id, popup.created_from_calendar))
        else {
            return;
        };
        if self.workspace.is_scheme_read_only(source_scheme_id)
            || self.workspace.is_scheme_read_only(target_scheme_id)
        {
            return;
        }
        if source_scheme_id == target_scheme_id {
            if let Some(popup) = self.event_popup.as_mut() {
                popup.scheme_menu_open = false;
            }
            cx.notify();
            return;
        }

        let Some(item) = self
            .workspace
            .scheme(source_scheme_id)
            .and_then(|s| s.item(item_id))
            .cloned()
        else {
            return;
        };

        let Some(position) = self
            .workspace
            .scheme(target_scheme_id)
            .map(|s| s.items.len())
        else {
            return;
        };

        // Keep the popup alive while reconciliation runs after the move batch.
        if let Some(popup) = self.event_popup.as_mut() {
            popup.scheme_id = target_scheme_id;
            popup.scheme_menu_open = false;
        }

        let command = Command::Batch(vec![
            Command::DeleteItem {
                scheme: source_scheme_id,
                item: item_id,
            },
            Command::InsertItem {
                scheme: target_scheme_id,
                position,
                item,
            },
        ]);
        let applied = if created_from_calendar {
            self.apply_without_pushing_undo(command, cx)
        } else {
            self.apply(command, cx)
        };
        if applied.is_some() {
            if created_from_calendar {
                self.retarget_pending_creation_undo(item_id, target_scheme_id);
            }
        } else if let Some(popup) = self.event_popup.as_mut() {
            popup.scheme_id = source_scheme_id;
        }
    }

    /// Resolve the end of a drag-to-move gesture. A negligible drag (no day
    /// change and no snapped time change) is treated as a plain click and opens
    /// the event popup — mirroring the old per-block `on_click`, which the
    /// gesture overlay now intercepts. Anything larger commits the move.
    pub(crate) fn finish_calendar_move(
        &mut self,
        mv: super::CalendarMoveState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mv.is_negligible() {
            self.open_event_popup(
                mv.scheme_id,
                mv.item_id,
                mv.occurrence,
                mv.occurrence_index,
                mv.occurrence_start,
                mv.occurrence_end,
                mv.anchor,
                false,
                false,
                window,
                cx,
            );
            return;
        }
        self.commit_calendar_move(mv, cx);
    }

    /// Commit a drag-to-move operation, applying time offset to the item's dates.
    pub(crate) fn commit_calendar_move(
        &mut self,
        mv: super::CalendarMoveState,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(mv.scheme_id) {
            return;
        }
        if mv.is_negligible() {
            // Negligible move — treat as a click (the title on_click will handle popup).
            return;
        }

        let Some(item) = self
            .workspace
            .scheme(mv.scheme_id)
            .and_then(|s| s.item(mv.item_id))
            .cloned()
        else {
            return;
        };

        // Same source of truth as the drag ghost — what was previewed is exactly
        // what we commit.
        let (draft_start, draft_end) = mv.draft_dates();
        let start_dirty = draft_start != mv.occurrence_start;
        let end_dirty = draft_end != mv.occurrence_end;
        if !start_dirty && !end_dirty {
            return;
        }

        if item.repeats.is_some() && !mv.occurrence.is_single() {
            self.close_date_popover();
            self.close_repeat_popover();
            let occurrence_state = item.state_for_occurrence(&mv.occurrence);
            let mut popup = EventPopup::new(
                mv.scheme_id,
                mv.item_id,
                &item,
                mv.occurrence.clone(),
                &occurrence_state,
                draft_start,
                draft_end,
                mv.anchor,
                mv.occurrence_index,
            );
            popup.start_dirty = start_dirty;
            popup.end_dirty = end_dirty;
            if let Some(Command::SetOccurrenceNotificationOffset { offset_secs, .. }) =
                reset_after_trigger_notification_to_default_command(
                    &item,
                    mv.scheme_id,
                    mv.item_id,
                    mv.occurrence.clone(),
                    draft_start,
                    draft_end,
                    Utc::now(),
                )
            {
                popup.draft_notification_offset_secs = offset_secs;
                popup.notification_dirty = true;
            }
            popup.scope_action = Some(EventScopeAction::ApplyChanges);
            popup.scope_dialog_only = true;
            self.event_popup = Some(popup);
            cx.notify();
            return;
        }

        let mut commands = Vec::new();
        if start_dirty {
            commands.push(Command::SetItemDate {
                scheme: mv.scheme_id,
                item: mv.item_id,
                kind: knotq_commands::DateKind::Start,
                date: draft_start,
            });
        }
        if end_dirty {
            commands.push(Command::SetItemDate {
                scheme: mv.scheme_id,
                item: mv.item_id,
                kind: knotq_commands::DateKind::End,
                date: draft_end,
            });
        }
        if let Some(command) = reset_after_trigger_notification_to_default_command(
            &item,
            mv.scheme_id,
            mv.item_id,
            mv.occurrence,
            draft_start,
            draft_end,
            Utc::now(),
        ) {
            commands.push(command);
        }

        if let Some(cmd) = Command::from_vec(commands) {
            self.apply(cmd, cx);
        }
    }

    /// Commit a bottom-edge resize operation, normalizing inverted endpoints.
    pub(crate) fn commit_calendar_resize(
        &mut self,
        resize: super::CalendarResizeState,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(resize.scheme_id) {
            return;
        }
        let proposed_end = snapped_calendar_datetime(resize.date, resize.current_hour);
        let (draft_start, draft_end) = if proposed_end < resize.occurrence_start {
            (proposed_end, resize.occurrence_start)
        } else {
            (resize.occurrence_start, proposed_end)
        };
        let draft_start = Some(draft_start);
        let draft_end = Some(draft_end);
        let start_dirty = draft_start != Some(resize.occurrence_start);
        let end_dirty = draft_end != Some(resize.occurrence_end);
        if !start_dirty && !end_dirty {
            return;
        }

        let Some(item) = self
            .workspace
            .scheme(resize.scheme_id)
            .and_then(|s| s.item(resize.item_id))
            .cloned()
        else {
            return;
        };

        if item.repeats.is_some() && !resize.occurrence.is_single() {
            self.close_date_popover();
            self.close_repeat_popover();
            let occurrence_state = item.state_for_occurrence(&resize.occurrence);
            let mut popup = EventPopup::new(
                resize.scheme_id,
                resize.item_id,
                &item,
                resize.occurrence,
                &occurrence_state,
                draft_start,
                draft_end,
                resize.anchor,
                resize.occurrence_index,
            );
            popup.start_dirty = start_dirty;
            popup.end_dirty = end_dirty;
            popup.scope_action = Some(EventScopeAction::ApplyChanges);
            popup.scope_dialog_only = true;
            self.event_popup = Some(popup);
            cx.notify();
            return;
        }

        let mut commands = Vec::new();
        if start_dirty {
            commands.push(Command::SetItemDate {
                scheme: resize.scheme_id,
                item: resize.item_id,
                kind: knotq_commands::DateKind::Start,
                date: draft_start,
            });
        }
        if end_dirty {
            commands.push(Command::SetItemDate {
                scheme: resize.scheme_id,
                item: resize.item_id,
                kind: knotq_commands::DateKind::End,
                date: draft_end,
            });
        }

        if let Some(cmd) = Command::from_vec(commands) {
            self.apply(cmd, cx);
        }
    }
}

fn date_edit_scope(scope: RepeatScope) -> DateEditScope {
    match scope {
        RepeatScope::ThisEvent => DateEditScope::ThisEvent,
        RepeatScope::AllFuture => DateEditScope::AllFuture,
        RepeatScope::AllEvents => DateEditScope::AllEvents,
    }
}
