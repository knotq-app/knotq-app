use std::collections::{HashSet, VecDeque};

use chrono::{DateTime, Utc};
use knotq_commands::{
    filter_recurring_occurrence_toggles, ChangeSet, Command, CommandOrigin, CommandReceipt,
    WorkspaceCommandExt,
};
use knotq_index::{IndexChangeSet, IndexedWorkspace};
use knotq_model::{OperationId, ReplicaId, SchemeId, Workspace, WorkspaceId};
use knotq_sync::{CrdtDocumentUpdate, PendingCrdtEdit};
use serde::{Deserialize, Serialize};

use crate::crdt::WorkspaceCrdtDocuments;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceDirtyState {
    pub schemes: HashSet<SchemeId>,
    pub index: bool,
}

impl WorkspaceDirtyState {
    pub fn from_parts(schemes: HashSet<SchemeId>, index: bool) -> Self {
        Self { schemes, index }
    }

    pub fn all(workspace: &Workspace) -> Self {
        Self {
            schemes: workspace.schemes.keys().copied().collect(),
            index: true,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.index || !self.schemes.is_empty()
    }

    pub fn clear(&mut self) {
        self.schemes.clear();
        self.index = false;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoreOperation {
    pub id: OperationId,
    pub workspace_id: WorkspaceId,
    pub replica_id: ReplicaId,
    pub sequence: u64,
    pub origin: CommandOrigin,
    pub created_at: DateTime<Utc>,
    pub command: Command,
    pub crdt_updates: Vec<CrdtDocumentUpdate>,
}

pub struct WorkspaceStore {
    workspace: Workspace,
    indexed: IndexedWorkspace,
    dirty: WorkspaceDirtyState,
    replica_id: ReplicaId,
    next_sequence: u64,
    pending_operations: VecDeque<StoreOperation>,
    crdt: WorkspaceCrdtDocuments,
}

impl WorkspaceStore {
    pub fn new(workspace: Workspace, replica_id: ReplicaId, initial_dirty: bool) -> Self {
        let mut workspace = workspace;
        let sync_metadata_dirty = workspace.ensure_sync_metadata();
        let mut dirty = if initial_dirty {
            WorkspaceDirtyState::all(&workspace)
        } else {
            WorkspaceDirtyState::default()
        };
        dirty.index |= sync_metadata_dirty;
        let indexed = IndexedWorkspace::build(workspace.clone());
        let crdt = WorkspaceCrdtDocuments::new(&workspace);
        Self {
            workspace,
            indexed,
            dirty,
            replica_id,
            next_sequence: 1,
            pending_operations: VecDeque::new(),
            crdt,
        }
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn indexed(&self) -> &IndexedWorkspace {
        &self.indexed
    }

    pub fn dirty(&self) -> &WorkspaceDirtyState {
        &self.dirty
    }

    pub fn replace_dirty_state(&mut self, dirty: WorkspaceDirtyState) {
        self.dirty = dirty;
    }

    pub fn pending_operations(&self) -> &VecDeque<StoreOperation> {
        &self.pending_operations
    }

    pub fn pending_crdt_edits(&self) -> Vec<PendingCrdtEdit> {
        self.pending_operations
            .iter()
            .flat_map(|operation| {
                operation
                    .crdt_updates
                    .iter()
                    .cloned()
                    .map(|update| PendingCrdtEdit {
                        operation_id: operation.id,
                        workspace_id: operation.workspace_id,
                        replica_id: operation.replica_id,
                        local_sequence: operation.sequence,
                        created_at: operation.created_at,
                        document: update.document,
                        kind: update.kind,
                        update_v1: update.update_v1,
                    })
            })
            .collect()
    }

    pub fn clear_pending_operations_through(&mut self, sequence: u64) -> usize {
        let before = self.pending_operations.len();
        while self
            .pending_operations
            .front()
            .is_some_and(|operation| operation.sequence <= sequence)
        {
            self.pending_operations.pop_front();
        }
        before - self.pending_operations.len()
    }

    pub fn replace_workspace(
        &mut self,
        workspace: Workspace,
        dirty: WorkspaceDirtyState,
        clear_pending_operations: bool,
    ) {
        let mut workspace = workspace;
        let sync_metadata_dirty = workspace.ensure_sync_metadata();
        let mut dirty = dirty;
        dirty.index |= sync_metadata_dirty;
        self.workspace = workspace.clone();
        self.indexed = IndexedWorkspace::build(workspace);
        self.crdt = WorkspaceCrdtDocuments::new(&self.workspace);
        self.dirty = dirty;
        if clear_pending_operations {
            self.pending_operations.clear();
        }
    }

    pub fn mark_dirty_from_command(&mut self, cmd: &Command) {
        self.dirty.index = true;
        collect_affected_schemes(cmd, &mut self.dirty.schemes);
    }

    pub fn mark_scheme_dirty(&mut self, scheme_id: SchemeId) {
        self.dirty.schemes.insert(scheme_id);
        self.dirty.index = true;
    }

    pub fn mark_index_dirty(&mut self) {
        self.dirty.index = true;
    }

    pub fn apply_local(
        &mut self,
        command: Command,
        origin: CommandOrigin,
    ) -> Result<Option<CommandReceipt>, knotq_commands::CommandError> {
        let Some(command) = filter_recurring_occurrence_toggles(command, &self.workspace) else {
            return Ok(None);
        };
        self.apply_prechecked_local(command, origin).map(Some)
    }

    pub fn apply_prechecked_local(
        &mut self,
        command: Command,
        origin: CommandOrigin,
    ) -> Result<CommandReceipt, knotq_commands::CommandError> {
        let receipt = self.workspace.apply(command.clone())?;
        let crdt_updates = self.after_workspace_change(&receipt.touched);
        self.pending_operations.push_back(StoreOperation {
            id: OperationId::new(),
            workspace_id: self.workspace.id,
            replica_id: self.replica_id,
            sequence: self.next_sequence,
            origin,
            created_at: Utc::now(),
            command,
            crdt_updates,
        });
        self.next_sequence += 1;
        Ok(receipt)
    }

    pub fn apply_remote(
        &mut self,
        command: Command,
    ) -> Result<Option<CommandReceipt>, knotq_commands::CommandError> {
        let Some(command) = filter_recurring_occurrence_toggles(command, &self.workspace) else {
            return Ok(None);
        };
        let receipt = self.workspace.apply(command)?;
        self.after_workspace_change(&receipt.touched);
        Ok(Some(receipt))
    }

    fn after_workspace_change(&mut self, changeset: &ChangeSet) -> Vec<CrdtDocumentUpdate> {
        for scheme_id in &changeset.schemes {
            self.dirty.schemes.insert(*scheme_id);
        }
        self.dirty.index = true;
        self.dirty.index |= self.workspace.ensure_sync_metadata();
        self.indexed.workspace = self.workspace.clone();
        self.indexed.apply_changeset(
            &IndexChangeSet {
                folders: changeset.folders.clone(),
                schemes: changeset.schemes.clone(),
            },
            &knotq_rrule::DefaultExpander,
        );
        self.crdt.sync_changes(&self.workspace, changeset)
    }
}

pub(crate) fn collect_affected_schemes(cmd: &Command, out: &mut HashSet<SchemeId>) {
    match cmd {
        Command::InsertItem { scheme, .. }
        | Command::UpdateItemText { scheme, .. }
        | Command::ReplaceItem { scheme, .. }
        | Command::SetItemIndent { scheme, .. }
        | Command::SetItemMarker { scheme, .. }
        | Command::SetItemDate { scheme, .. }
        | Command::SetItemRecurrence { scheme, .. }
        | Command::SetItemPriority { scheme, .. }
        | Command::SetOccurrenceNotificationOffset { scheme, .. }
        | Command::ToggleOccurrence { scheme, .. }
        | Command::DeleteItem { scheme, .. }
        | Command::ReorderItem { scheme, .. }
        | Command::RenameScheme { id: scheme, .. }
        | Command::SetSchemeColor { id: scheme, .. }
        | Command::SetSchemeGsync { id: scheme, .. }
        | Command::SetSchemeSource { id: scheme, .. }
        | Command::DeleteScheme { id: scheme }
        | Command::PermanentlyDeleteScheme { id: scheme } => {
            out.insert(*scheme);
        }
        Command::RestoreScheme { scheme, .. } | Command::RestoreDeletedScheme { scheme, .. } => {
            out.insert(scheme.id);
        }
        Command::Batch(cmds) => {
            for cmd in cmds {
                collect_affected_schemes(cmd, out);
            }
        }
        Command::CreateFolder { .. }
        | Command::RestoreFolder { .. }
        | Command::RenameFolder { .. }
        | Command::SetFolderExpanded { .. }
        | Command::DeleteFolder { .. }
        | Command::CreateScheme { .. }
        | Command::MoveNode { .. } => {}
    }
}
