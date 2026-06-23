use gpui::Context;
use knotq_commands::{Command, CommandOrigin};
use knotq_model::{ItemId, SchemeId};

use crate::app::{
    calendar_toggle_keys, EditorUndoKey, KnotQApp, UndoNavigationEntry, UNDO_DEPTH,
};

use super::{pending_creation_undo_matches, service_signals_for_command};

impl KnotQApp {
    pub(crate) fn retarget_pending_creation_undo(
        &mut self,
        item_id: ItemId,
        target_scheme_id: SchemeId,
    ) {
        if let Some(Command::DeleteItem { scheme, item }) = self.undo_stack.back_mut() {
            if *item == item_id {
                *scheme = target_scheme_id;
            }
        }
    }

    pub(crate) fn discard_pending_creation_undo(&mut self, item_id: ItemId) -> bool {
        if !pending_creation_undo_matches(self.undo_stack.back(), item_id) {
            return false;
        }
        self.undo_stack.pop_back();
        self.undo_navigation_stack.pop_back();
        true
    }

    pub(crate) fn item_allows_occurrence_toggle(
        &self,
        scheme_id: SchemeId,
        item_id: ItemId,
        occurrence: &knotq_model::OccurrenceId,
    ) -> bool {
        self.workspace
            .scheme(scheme_id)
            .and_then(|scheme| scheme.item(item_id))
            .is_some_and(|item| item.repeats.is_none() || !occurrence.is_single())
    }

    pub(super) fn active_repeat_popover_undo_key(&self) -> Option<EditorUndoKey> {
        self.repeat_popover.as_ref().map(|popup| EditorUndoKey {
            scheme_id: popup.scheme_id,
            item_id: popup.item_id,
        })
    }

    pub(super) fn push_undo(&mut self, inverse: Command, navigation: UndoNavigationEntry) {
        self.undo_stack.push_back(inverse);
        self.undo_navigation_stack.push_back(navigation);
        while self.undo_stack.len() > UNDO_DEPTH {
            self.undo_stack.pop_front();
            self.undo_navigation_stack.pop_front();
        }
        while self.undo_navigation_stack.len() > self.undo_stack.len() {
            self.undo_navigation_stack.pop_front();
        }
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        if let Some(inv) = self.undo_stack.pop_back() {
            let completed_clear_cmd = inv.clone();
            let navigation = self.undo_navigation_stack.pop_back();
            let toggled = calendar_toggle_keys(&inv);
            let service_signals = service_signals_for_command(&inv, &self.workspace);
            if let Ok(receipt) = self.apply_workspace_store_command(inv, CommandOrigin::User) {
                self.sync_retained_completed_calendar_items(&toggled);
                self.clear_completed_occurrence_notifications(&completed_clear_cmd);
                self.redo_stack.push_back(receipt.inverse);
                if let Some(navigation) = navigation.as_ref() {
                    self.redo_navigation_stack.push_back(navigation.clone());
                }
                self.reconcile_workspace_ui_state();
                if let Some(navigation) = navigation {
                    self.restore_undo_navigation_snapshot(&navigation.before, cx);
                }
                self.signal_workspace_services(service_signals);
                cx.notify();
            }
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        if let Some(inv) = self.redo_stack.pop_back() {
            let completed_clear_cmd = inv.clone();
            let navigation = self.redo_navigation_stack.pop_back();
            let toggled = calendar_toggle_keys(&inv);
            let service_signals = service_signals_for_command(&inv, &self.workspace);
            if let Ok(receipt) = self.apply_workspace_store_command(inv, CommandOrigin::User) {
                self.sync_retained_completed_calendar_items(&toggled);
                self.clear_completed_occurrence_notifications(&completed_clear_cmd);
                if let Some(navigation) = navigation.as_ref() {
                    self.push_undo(receipt.inverse, navigation.clone());
                } else {
                    self.undo_stack.push_back(receipt.inverse);
                }
                self.reconcile_workspace_ui_state();
                if let Some(navigation) = navigation {
                    self.restore_undo_navigation_snapshot(&navigation.after, cx);
                }
                self.signal_workspace_services(service_signals);
                cx.notify();
            }
        }
    }
}
