use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use knotq_commands::{
    filter_recurring_occurrence_toggles, ChangeSet, Command, CommandOrigin, CommandReceipt,
    WorkspaceCommandExt,
};
use knotq_index::{IndexChangeSet, IndexedWorkspace};
use knotq_model::{
    DocumentId, OperationId, ReplicaId, SchemeId, SyncDocumentKind, Workspace, WorkspaceId,
};
use knotq_sync::{
    validate_crdt_update_sequence, CrdtDocumentUpdate, PendingCrdtEdit, StoredCrdtUpdate,
    WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments,
};
use serde::{Deserialize, Serialize};

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
    pub fn new(
        workspace: Workspace,
        replica_id: ReplicaId,
        initial_dirty: bool,
        crdt_states: HashMap<DocumentId, Vec<u8>>,
        initial_sequence: u64,
    ) -> Self {
        let mut workspace = workspace;
        let sync_metadata_dirty = workspace.ensure_sync_metadata();
        let mut dirty = if initial_dirty {
            WorkspaceDirtyState::all(&workspace)
        } else {
            WorkspaceDirtyState::default()
        };
        dirty.index |= sync_metadata_dirty;
        let indexed = IndexedWorkspace::build(workspace.clone());
        let crdt = restored_workspace_crdt(&workspace, replica_id, &crdt_states);
        Self {
            workspace,
            indexed,
            dirty,
            replica_id,
            next_sequence: initial_sequence.max(1),
            pending_operations: VecDeque::new(),
            crdt,
        }
    }

    /// Snapshot the long-lived CRDT documents' state for durable persistence and to
    /// seed the background sync's CRDT from this device's latest local edits.
    pub fn crdt_document_states(&self) -> HashMap<DocumentId, Vec<u8>> {
        self.crdt.document_states()
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

    pub fn has_pending_crdt_edits(&self) -> bool {
        self.pending_operations
            .iter()
            .any(|op| !op.crdt_updates.is_empty())
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

    pub fn clear_pushed_crdt_edits(
        &mut self,
        document: DocumentId,
        through_local_sequence: u64,
    ) -> usize {
        let mut cleared = 0;
        for operation in &mut self.pending_operations {
            if operation.sequence > through_local_sequence {
                continue;
            }
            let before = operation.crdt_updates.len();
            operation
                .crdt_updates
                .retain(|update| update.document != document);
            cleared += before - operation.crdt_updates.len();
        }
        self.pending_operations
            .retain(|operation| !operation.crdt_updates.is_empty());
        cleared
    }

    /// Replace the workspace while preserving the CRDT documents' stable Yjs identity
    /// (clientID + clocks). The CRDT is reconstructed from its own current state, so a
    /// direct (non-command) workspace mutation never mints a throwaway identity that
    /// would diverge under sync.
    pub fn replace_workspace(
        &mut self,
        workspace: Workspace,
        dirty: WorkspaceDirtyState,
        clear_pending_operations: bool,
    ) {
        let states = self.crdt.document_states();
        let direct_changes = WorkspaceCrdtChangeSet {
            workspace: dirty.index,
            schemes: dirty.schemes.clone(),
        };
        self.replace_workspace_with_crdt_states(workspace, dirty, clear_pending_operations, states);
        self.record_direct_crdt_changes(direct_changes);
    }

    /// Direct (non-command) workspace mutations — e.g. creating today's Daily Queue
    /// scheme — reach the store only through [`replace_workspace`](Self::replace_workspace).
    /// The rebuilt CRDT preserves prior document state, so the mutation itself is
    /// not yet in any document; sync the dirty change set into the CRDT and queue
    /// the resulting updates exactly as a command would. Without this, a brand-new
    /// scheme's document stays empty and its first push is rejected as
    /// `crdt_schema_invalid`, wedging the sync queue. Changes already recorded by
    /// the command path diff to nothing here, so this only emits genuinely
    /// unrecorded edits.
    fn record_direct_crdt_changes(&mut self, changes: WorkspaceCrdtChangeSet) {
        if !changes.workspace && changes.schemes.is_empty() {
            return;
        }
        let outcome = self.crdt.sync_changes(&self.workspace, &changes);
        for error in &outcome.errors {
            eprintln!("CRDT direct sync update failed: {error}");
        }
        if outcome.updates.is_empty() {
            return;
        }
        self.pending_operations.push_back(StoreOperation {
            id: OperationId::new(),
            workspace_id: self.workspace.id,
            replica_id: self.replica_id,
            sequence: self.next_sequence,
            origin: CommandOrigin::User,
            created_at: Utc::now(),
            command: Command::Batch(Vec::new()),
            crdt_updates: outcome.updates,
        });
        self.next_sequence += 1;
    }

    /// Replace the workspace and rebuild the CRDT documents from the given persisted
    /// `crdt_states` (deterministic clientID). Used after a sync merges remote state:
    /// the store adopts the merged documents' canonical identity rather than
    /// re-seeding its own.
    pub fn replace_workspace_with_crdt_states(
        &mut self,
        workspace: Workspace,
        dirty: WorkspaceDirtyState,
        clear_pending_operations: bool,
        crdt_states: HashMap<DocumentId, Vec<u8>>,
    ) {
        let mut workspace = workspace;
        let sync_metadata_dirty = workspace.ensure_sync_metadata();
        let mut dirty = dirty;
        dirty.index |= sync_metadata_dirty;
        self.workspace = workspace.clone();
        self.indexed = IndexedWorkspace::build(workspace);
        self.crdt = restored_workspace_crdt(&self.workspace, self.replica_id, &crdt_states);
        self.dirty = dirty;
        if clear_pending_operations {
            self.pending_operations.clear();
        }
    }

    /// Monotonic watermark of locally applied operations. Capture it when a
    /// background sync run snapshots the workspace and compare on completion to
    /// detect edits applied while the run's network round trip was in flight.
    pub fn local_sequence_watermark(&self) -> u64 {
        self.next_sequence
    }

    /// Merge a completed sync run's final document states into the live CRDT
    /// documents instead of replacing them. The run worked on a copy seeded from
    /// a snapshot taken when it started, so its result lacks any edit applied
    /// while its network round trip was in flight; a wholesale replace would
    /// roll those edits back and dismiss UI anchored to them (e.g. an event
    /// popup whose just-created item vanishes from the workspace). Full Yjs
    /// states are valid updates, so applying them to the live documents yields
    /// the union of the remote changes and the in-flight local edits.
    ///
    /// Returns false — leaving the documents for the caller's replace fallback —
    /// when the merged workspace fails validation or a document reports a
    /// non-benign apply error.
    pub fn merge_sync_crdt_states(
        &mut self,
        sync_workspace: &Workspace,
        crdt_states: &HashMap<DocumentId, Vec<u8>>,
    ) -> bool {
        let received_at = Utc::now();
        let updates = crdt_states
            .iter()
            .filter_map(|(document, state)| {
                let kind = if *document == sync_workspace.sync.id {
                    SyncDocumentKind::PersonalWorkspace
                } else {
                    SyncDocumentKind::Scheme
                };
                // A schema-less state is an empty document (e.g. a scheme that
                // was never edited or pulled on either side); it contributes
                // nothing and applying it would only trip post-apply schema
                // validation, so skip it.
                validate_crdt_update_sequence(kind, [state.as_slice()]).ok()?;
                Some(StoredCrdtUpdate {
                    workspace_id: sync_workspace.id,
                    document: *document,
                    kind,
                    replica_id: self.replica_id,
                    sequence: 0,
                    received_at,
                    update_v1: state.clone(),
                })
            })
            .collect::<Vec<_>>();
        let outcome = self.crdt.apply_remote_updates(&self.workspace, &updates);
        for error in &outcome.workspace_errors {
            eprintln!("sync merge workspace error: {}", error.message);
        }
        let mut mergeable = outcome.workspace_is_ok();
        for error in &outcome.document_errors {
            // "Unknown scheme document" is benign here: the run's result still
            // carries a content document for a scheme deleted locally mid-run;
            // the merged index (where the local delete won) routes nothing to it.
            if error.unknown_scheme_document {
                continue;
            }
            eprintln!("sync merge document error: {}", error.message);
            mergeable = false;
        }
        if !mergeable {
            return false;
        }
        self.workspace = outcome.workspace.clone();
        self.indexed = IndexedWorkspace::build(outcome.workspace);
        self.dirty = WorkspaceDirtyState::all(&self.workspace);
        true
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
        let crdt_changes = crdt_change_set_for_command(&command);
        let crdt_updates = self.after_workspace_change(&receipt.touched, crdt_changes);
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
        let crdt_changes = crdt_change_set_for_command(&command);
        let receipt = self.workspace.apply(command)?;
        self.after_workspace_change(&receipt.touched, crdt_changes);
        Ok(Some(receipt))
    }

    fn after_workspace_change(
        &mut self,
        changeset: &ChangeSet,
        mut crdt_changes: WorkspaceCrdtChangeSet,
    ) -> Vec<CrdtDocumentUpdate> {
        for scheme_id in &changeset.schemes {
            self.dirty.schemes.insert(*scheme_id);
        }
        self.dirty.index = true;
        if self.workspace.ensure_sync_metadata() {
            self.dirty.index = true;
            crdt_changes.workspace = true;
        }
        self.indexed.workspace = self.workspace.clone();
        self.indexed.apply_changeset(
            &IndexChangeSet {
                folders: changeset.folders.clone(),
                schemes: changeset.schemes.clone(),
            },
            &knotq_rrule::DefaultExpander,
        );
        let outcome = self.crdt.sync_changes(&self.workspace, &crdt_changes);
        for error in &outcome.errors {
            eprintln!("CRDT sync update failed: {error}");
        }
        outcome.updates
    }
}

/// Restore the long-lived CRDT documents from persisted `crdt_states` with a stable,
/// deterministic clientID for this replica. Documents absent from `crdt_states` are
/// left empty and populated by the next sync (adopting the server's canonical
/// identity) or force-emitted as a full snapshot on the next local edit — never
/// rebuilt from plain data with a throwaway identity.
fn restored_workspace_crdt(
    workspace: &Workspace,
    replica_id: ReplicaId,
    crdt_states: &HashMap<DocumentId, Vec<u8>>,
) -> WorkspaceCrdtDocuments {
    match WorkspaceCrdtDocuments::from_states(workspace, replica_id, crdt_states) {
        Ok(crdt) => crdt,
        Err(err) => {
            eprintln!("restore CRDT documents failed: {err:#}");
            WorkspaceCrdtDocuments::empty_for_replica(workspace, replica_id)
        }
    }
}

fn crdt_change_set_for_command(command: &Command) -> WorkspaceCrdtChangeSet {
    let mut changes = WorkspaceCrdtChangeSet::default();
    collect_crdt_changes(command, &mut changes);
    changes
}

fn collect_crdt_changes(command: &Command, out: &mut WorkspaceCrdtChangeSet) {
    match command {
        Command::CreateFolder { .. }
        | Command::RestoreFolder { .. }
        | Command::RenameFolder { .. }
        | Command::SetFolderExpanded { .. }
        | Command::DeleteFolder { .. }
        | Command::PermanentlyDeleteFolder { .. }
        | Command::CreateScheme { .. }
        | Command::RenameScheme { .. }
        | Command::SetSchemeColor { .. }
        | Command::SetSchemeGsync { .. }
        | Command::SetSchemeSource { .. }
        | Command::DeleteScheme { .. }
        | Command::PermanentlyDeleteScheme { .. }
        | Command::MoveNode { .. } => {
            out.workspace = true;
        }
        Command::RestoreScheme { scheme, .. } | Command::RestoreDeletedScheme { scheme, .. } => {
            out.workspace = true;
            out.schemes.insert(scheme.id);
        }
        Command::RestoreDeletedFolder { schemes, .. } => {
            out.workspace = true;
            for scheme in schemes {
                out.schemes.insert(scheme.id);
            }
        }
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
        | Command::ReorderItem { scheme, .. } => {
            out.schemes.insert(*scheme);
        }
        Command::Batch(commands) => {
            for command in commands {
                collect_crdt_changes(command, out);
            }
        }
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
        Command::RestoreDeletedFolder { schemes, .. } => {
            for scheme in schemes {
                out.insert(scheme.id);
            }
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
        | Command::PermanentlyDeleteFolder { .. }
        | Command::CreateScheme { .. }
        | Command::MoveNode { .. } => {}
    }
}
