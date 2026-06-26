use gpui::Context;
use knotq_commands::{Command, CommandOrigin};
use knotq_model::{ItemId, SchemeId};

use crate::app::{calendar_toggle_keys, EditorUndoKey, KnotQApp, UndoEntry};

use super::{pending_creation_undo_matches, primary_cursor_item, service_signals_for_command};

impl KnotQApp {
    pub(crate) fn retarget_pending_creation_undo(
        &mut self,
        item_id: ItemId,
        target_scheme_id: SchemeId,
    ) {
        if let Some(entry) = self.undo_store.last_undo_mut() {
            if let Command::DeleteItem { scheme, item } = &mut entry.inverse {
                if *item == item_id {
                    *scheme = target_scheme_id;
                }
            }
        }
    }

    pub(crate) fn discard_pending_creation_undo(&mut self, item_id: ItemId) -> bool {
        if !pending_creation_undo_matches(self.undo_store.last_undo().map(|e| &e.inverse), item_id) {
            return false;
        }
        self.undo_store.discard_last_undo();
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

    pub(super) fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_store.push_undo(entry);
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        let scope = self.active_undo_scope();
        // Skip entries whose inverse no longer applies (invalidated by a later
        // edit or sync); applying is atomic, so a skip leaves state untouched.
        while let Some(entry) = self.undo_store.take_undo(scope) {
            let inv = entry.inverse;
            let completed_clear_cmd = inv.clone();
            let toggled = calendar_toggle_keys(&inv);
            let cursor_target = primary_cursor_item(&inv);
            let service_signals = service_signals_for_command(&inv, &self.workspace);
            match self.apply_workspace_store_command(inv, CommandOrigin::User) {
                Ok(receipt) => {
                    self.sync_retained_completed_calendar_items(&toggled);
                    self.clear_completed_occurrence_notifications(&completed_clear_cmd);
                    self.undo_store.record_redo(UndoEntry {
                        inverse: receipt.inverse,
                        scope: entry.scope,
                        before: entry.before.clone(),
                        after: entry.after.clone(),
                    });
                    self.reconcile_workspace_ui_state();
                    self.restore_undo_navigation_snapshot(&entry.before, cx);
                    self.place_cursor_after_undo(cursor_target);
                    self.signal_workspace_services(service_signals);
                    cx.notify();
                    return;
                }
                Err(_) => continue,
            }
        }
    }

    /// Request the editor place its caret on the item a just-undone/redone
    /// command changed, so the user lands where the change happened. Only acts
    /// when that item is in the scheme now in view (the common case — a scheme
    /// edit undone while its scheme is focused); `focused_item_id` is a one-shot
    /// request the editor render consumes via `focus_item`.
    fn place_cursor_after_undo(&mut self, target: Option<(SchemeId, ItemId)>) {
        if let Some((scheme, item)) = target {
            if self.selection.scheme_id == Some(scheme) && self.scheme_item_exists(scheme, item) {
                self.selection.focused_item_id = Some(item);
            }
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        let scope = self.active_undo_scope();
        while let Some(entry) = self.undo_store.take_redo(scope) {
            let inv = entry.inverse;
            let completed_clear_cmd = inv.clone();
            let toggled = calendar_toggle_keys(&inv);
            let cursor_target = primary_cursor_item(&inv);
            let service_signals = service_signals_for_command(&inv, &self.workspace);
            match self.apply_workspace_store_command(inv, CommandOrigin::User) {
                Ok(receipt) => {
                    self.sync_retained_completed_calendar_items(&toggled);
                    self.clear_completed_occurrence_notifications(&completed_clear_cmd);
                    self.push_undo(UndoEntry {
                        inverse: receipt.inverse,
                        scope: entry.scope,
                        before: entry.before.clone(),
                        after: entry.after.clone(),
                    });
                    self.reconcile_workspace_ui_state();
                    self.restore_undo_navigation_snapshot(&entry.after, cx);
                    self.place_cursor_after_undo(cursor_target);
                    self.signal_workspace_services(service_signals);
                    cx.notify();
                    return;
                }
                Err(_) => continue,
            }
        }
    }
}
