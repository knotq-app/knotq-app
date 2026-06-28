use gpui::Context;
use knotq_commands::{event_popup_commit_commands, Command, DateEditScope, EventPopupDraft};

use crate::app::{EventPopup, EventScopeAction, KnotQApp, RepeatScope};

impl KnotQApp {
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
        // On a read-only (imported) scheme only the completion toggle is allowed
        // through — its content and schedule can't be edited locally — so the
        // other draft changes are dropped rather than committed.
        let read_only = self.workspace.is_scheme_read_only(popup.scheme_id);

        let mut commands = Vec::new();
        if !read_only && popup.title_dirty && item.text() != popup.draft_title {
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
            start_dirty: !read_only && popup.start_dirty,
            end_dirty: !read_only && popup.end_dirty,
            repeats_dirty: !read_only && popup.repeats_dirty,
            notification_dirty: !read_only && popup.notification_dirty,
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
}

fn date_edit_scope(scope: RepeatScope) -> DateEditScope {
    match scope {
        RepeatScope::ThisEvent => DateEditScope::ThisEvent,
        RepeatScope::AllFuture => DateEditScope::AllFuture,
        RepeatScope::AllEvents => DateEditScope::AllEvents,
    }
}
