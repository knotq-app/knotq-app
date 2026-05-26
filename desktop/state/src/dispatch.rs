use std::time::Instant;

use knotq_commands::{
    filter_recurring_occurrence_toggles, ChangeSet, Command, CommandOrigin, CommandReceipt,
};

use crate::{
    editor_undo_key, recurrence_undo_key, should_coalesce_editor_undo, AppEvent, AppState,
    EditorUndoGroup,
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
        let command = self
            .undo
            .pop_undo()
            .or_else(|| self.undo_stack.pop_back())?;
        self.sync_store_from_compat();
        let receipt = self
            .store
            .apply_prechecked_local(command, CommandOrigin::User)
            .ok()?;
        self.sync_compat_from_store();
        self.after_workspace_change(&receipt.touched);
        self.undo.push_redo(receipt.inverse.clone());
        self.redo_stack.push_back(receipt.inverse.clone());
        Some(receipt)
    }

    pub fn redo_command(&mut self) -> Option<CommandReceipt> {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        let command = self
            .undo
            .pop_redo()
            .or_else(|| self.redo_stack.pop_back())?;
        self.sync_store_from_compat();
        let receipt = self
            .store
            .apply_prechecked_local(command, CommandOrigin::User)
            .ok()?;
        self.sync_compat_from_store();
        self.after_workspace_change(&receipt.touched);
        self.undo.push_undo(receipt.inverse.clone());
        self.undo_stack.push_back(receipt.inverse.clone());
        Some(receipt)
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
        self.sync_store_from_compat();
        let receipt = self
            .store
            .apply_prechecked_local(command, CommandOrigin::User)
            .ok()?;
        self.sync_compat_from_store();
        if !coalesce {
            self.undo.push_undo(receipt.inverse.clone());
            self.undo_stack.push_back(receipt.inverse.clone());
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
        self.undo.clear_redo();
        self.redo_stack.clear();
        self.after_workspace_change(&receipt.touched);
        Some(receipt)
    }

    fn after_workspace_change(&mut self, changeset: &ChangeSet) {
        self.event_bus
            .emit(AppEvent::WorkspaceChanged(changeset.clone()));
    }
}
