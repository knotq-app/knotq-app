use std::time::Instant;

use gpui::Context;
use knotq_commands::{
    filter_recurring_occurrence_toggles, Command, CommandError, CommandOrigin, CommandReceipt,
};

use crate::app::{
    calendar_toggle_keys, editor_undo_key, recurrence_undo_key, should_coalesce_editor_undo,
    should_coalesce_recurrence_undo, EditorUndoGroup, KnotQApp, UndoNavigationEntry,
};

use super::service_signals_for_command;

impl KnotQApp {
    /// Apply a command, push its inverse onto the undo stack, and mark dirty.
    pub fn apply(&mut self, cmd: Command, cx: &mut Context<Self>) -> Option<CommandReceipt> {
        match self.apply_result(cmd, cx) {
            Ok(receipt) => receipt,
            Err(err) => {
                eprintln!("command failed: {err}");
                None
            }
        }
    }

    pub(crate) fn apply_result(
        &mut self,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> Result<Option<CommandReceipt>, CommandError> {
        self.editor_undo_group = None;
        let Some(cmd) = filter_recurring_occurrence_toggles(cmd, &self.workspace) else {
            self.recurrence_undo_group = None;
            return Ok(None);
        };
        let recurrence_key = recurrence_undo_key(&cmd);
        let coalesce_recurrence = should_coalesce_recurrence_undo(
            recurrence_key,
            self.recurrence_undo_group,
            self.active_repeat_popover_undo_key(),
        );
        let nav_before = self.undo_navigation_snapshot();
        let toggled = calendar_toggle_keys(&cmd);
        let service_signals = service_signals_for_command(&cmd, &self.workspace);
        self.clear_deleted_item_notifications(&cmd);
        let completed_clear_cmd = cmd.clone();
        match self.apply_workspace_store_command(cmd, CommandOrigin::User) {
            Ok(receipt) => {
                self.sync_retained_completed_calendar_items(&toggled);
                self.clear_completed_occurrence_notifications(&completed_clear_cmd);
                self.recurrence_undo_group = recurrence_key.map(|key| EditorUndoGroup {
                    key,
                    last_edit: Instant::now(),
                });
                self.redo_stack.clear();
                self.reconcile_workspace_ui_state();
                let nav_after = self.undo_navigation_snapshot();
                if !coalesce_recurrence {
                    self.push_undo(
                        receipt.inverse.clone(),
                        UndoNavigationEntry {
                            before: nav_before,
                            after: nav_after,
                        },
                    );
                }
                self.redo_navigation_stack.clear();
                self.signal_workspace_services(service_signals);
                cx.notify();
                Ok(Some(receipt))
            }
            Err(err) => Err(err),
        }
    }

    /// Apply a command as part of an existing undoable user action.
    pub(crate) fn apply_without_pushing_undo(
        &mut self,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> Option<CommandReceipt> {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        let Some(cmd) = filter_recurring_occurrence_toggles(cmd, &self.workspace) else {
            return None;
        };
        let toggled = calendar_toggle_keys(&cmd);
        let service_signals = service_signals_for_command(&cmd, &self.workspace);
        self.clear_deleted_item_notifications(&cmd);
        let completed_clear_cmd = cmd.clone();
        match self.apply_workspace_store_command(cmd, CommandOrigin::User) {
            Ok(receipt) => {
                self.sync_retained_completed_calendar_items(&toggled);
                self.clear_completed_occurrence_notifications(&completed_clear_cmd);
                self.redo_stack.clear();
                self.redo_navigation_stack.clear();
                self.reconcile_workspace_ui_state();
                self.signal_workspace_services(service_signals);
                cx.notify();
                Some(receipt)
            }
            Err(err) => {
                eprintln!("command failed: {err}");
                None
            }
        }
    }

    /// Like `apply` but coalesces consecutive text edits on the same item into
    /// a single undo entry when they occur within the grouping window.
    pub(crate) fn apply_editor_command(
        &mut self,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> Option<CommandReceipt> {
        self.recurrence_undo_group = None;
        let Some(cmd) = filter_recurring_occurrence_toggles(cmd, &self.workspace) else {
            self.editor_undo_group = None;
            return None;
        };
        let now = Instant::now();
        let key = editor_undo_key(&cmd);
        let coalesce = should_coalesce_editor_undo(key, self.editor_undo_group, now);
        let nav_before = self.undo_navigation_snapshot();
        let toggled = calendar_toggle_keys(&cmd);
        let service_signals = service_signals_for_command(&cmd, &self.workspace);
        self.clear_deleted_item_notifications(&cmd);
        let completed_clear_cmd = cmd.clone();
        match self.apply_workspace_store_command(cmd, CommandOrigin::User) {
            Ok(receipt) => {
                self.sync_retained_completed_calendar_items(&toggled);
                self.clear_completed_occurrence_notifications(&completed_clear_cmd);
                self.editor_undo_group = key.map(|key| EditorUndoGroup {
                    key,
                    last_edit: now,
                });
                self.redo_stack.clear();
                self.reconcile_workspace_ui_state();
                let nav_after = self.undo_navigation_snapshot();
                if !coalesce {
                    self.push_undo(
                        receipt.inverse.clone(),
                        UndoNavigationEntry {
                            before: nav_before,
                            after: nav_after,
                        },
                    );
                }
                self.redo_navigation_stack.clear();
                self.signal_workspace_services(service_signals);
                cx.notify();
                Some(receipt)
            }
            Err(err) => {
                self.editor_undo_group = None;
                eprintln!("editor command failed: {err}");
                None
            }
        }
    }

    pub(super) fn apply_workspace_store_command(
        &mut self,
        cmd: Command,
        origin: CommandOrigin,
    ) -> Result<CommandReceipt, CommandError> {
        self.state.apply_prechecked_local_command(cmd, origin)
    }
}
