use std::time::Instant;

use knotq_commands::{
    filter_recurring_occurrence_toggles, ChangeSet, Command, CommandOrigin, CommandReceipt,
};

use crate::{
    editor_undo_key, recurrence_undo_key, should_coalesce_editor_undo, AppEvent, AppState,
    EditorUndoGroup, NavSnapshot, UndoEntry,
};

pub struct CommandDispatcher<'a> {
    state: &'a mut AppState,
}

impl<'a> CommandDispatcher<'a> {
    pub fn new(state: &'a mut AppState) -> Self {
        Self { state }
    }

    pub fn apply(&mut self, command: Command) -> Option<CommandReceipt> {
        self.state.apply_command(command)
    }

    pub fn apply_editor_command(&mut self, command: Command) -> Option<CommandReceipt> {
        self.state.apply_editor_command(command)
    }

    pub fn undo(&mut self) -> Option<CommandReceipt> {
        self.state.undo_command()
    }

    pub fn redo(&mut self) -> Option<CommandReceipt> {
        self.state.redo_command()
    }
}

impl AppState {
    pub fn dispatcher(&mut self) -> CommandDispatcher<'_> {
        CommandDispatcher::new(self)
    }

    pub fn apply_command(&mut self, command: Command) -> Option<CommandReceipt> {
        self.editor_undo_group = None;
        self.apply_filtered_command(command, false, None)
    }

    pub fn apply_editor_command(&mut self, command: Command) -> Option<CommandReceipt> {
        self.recurrence_undo_group = None;
        let now = Instant::now();
        self.apply_filtered_command(command, true, Some(now))
    }

    pub fn undo_command(&mut self) -> Option<CommandReceipt> {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        let scope = self.active_undo_scope();
        // Skip entries whose inverse no longer applies (a later edit or sync
        // invalidated it); applying is atomic, so a skipped entry leaves the
        // workspace untouched.
        while let Some(entry) = self.undo_store.take_undo(scope) {
            self.sync_store_from_workspace();
            match self
                .store
                .apply_prechecked_local(entry.inverse, CommandOrigin::User)
            {
                Ok(receipt) => {
                    self.sync_workspace_from_store();
                    self.after_workspace_change(&receipt.touched);
                    self.undo_store.record_redo(UndoEntry {
                        inverse: receipt.inverse.clone(),
                        scope: entry.scope,
                        before: entry.before,
                        after: entry.after,
                    });
                    return Some(receipt);
                }
                Err(_) => continue,
            }
        }
        None
    }

    pub fn redo_command(&mut self) -> Option<CommandReceipt> {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        let scope = self.active_undo_scope();
        while let Some(entry) = self.undo_store.take_redo(scope) {
            self.sync_store_from_workspace();
            match self
                .store
                .apply_prechecked_local(entry.inverse, CommandOrigin::User)
            {
                Ok(receipt) => {
                    self.sync_workspace_from_store();
                    self.after_workspace_change(&receipt.touched);
                    self.undo_store.push_undo(UndoEntry {
                        inverse: receipt.inverse.clone(),
                        scope: entry.scope,
                        before: entry.before,
                        after: entry.after,
                    });
                    return Some(receipt);
                }
                Err(_) => continue,
            }
        }
        None
    }

    fn apply_filtered_command(
        &mut self,
        command: Command,
        editor_command: bool,
        now: Option<Instant>,
    ) -> Option<CommandReceipt> {
        let command = filter_recurring_occurrence_toggles(command, &self.workspace)?;
        let coalesce = if editor_command {
            should_coalesce_editor_undo(editor_undo_key(&command), self.editor_undo_group, now?)
        } else {
            false
        };
        let recurrence_key = recurrence_undo_key(&command);
        let scope = self.undo_scope_for(&command);
        let before = self.nav_snapshot();
        self.sync_store_from_workspace();
        let receipt = self
            .store
            .apply_prechecked_local(command, CommandOrigin::User)
            .ok()?;
        self.sync_workspace_from_store();
        if !coalesce {
            let after = self.nav_snapshot();
            self.undo_store.push_undo(UndoEntry {
                inverse: receipt.inverse.clone(),
                scope,
                before,
                after,
            });
        }
        if editor_command {
            self.editor_undo_group = editor_undo_key(&receipt.inverse).map(|key| EditorUndoGroup {
                key,
                last_edit: now.unwrap_or_else(Instant::now),
            });
        } else {
            self.recurrence_undo_group = recurrence_key.map(|key| EditorUndoGroup {
                key,
                last_edit: Instant::now(),
            });
        }
        self.undo_store.clear_redo_conflicting(&receipt.inverse);
        self.after_workspace_change(&receipt.touched);
        Some(receipt)
    }

    fn nav_snapshot(&self) -> NavSnapshot {
        NavSnapshot {
            selection: self.selection.clone(),
            week_offset: self.week_offset,
            month_offset: self.month_offset,
        }
    }

    fn after_workspace_change(&mut self, changeset: &ChangeSet) {
        self.event_bus
            .emit(AppEvent::WorkspaceChanged(changeset.clone()));
    }
}
