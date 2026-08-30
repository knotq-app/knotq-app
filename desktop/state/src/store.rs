use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use knotq_commands::{
    filter_recurring_occurrence_toggles, ChangeSet, Command, CommandOrigin, CommandReceipt,
    WorkspaceCommandExt,
};
use knotq_index::IndexedWorkspace;
use knotq_model::{
    DocumentId, OperationId, ReplicaId, SchemeId, SyncDocumentKind, Workspace, WorkspaceId,
};
use knotq_sync::{
    validate_crdt_update_sequence, CrdtDocumentUpdate, DocumentStateHandle, PendingCrdtEdit,
    StoredCrdtUpdate, WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments,
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

/// Which CRDT document state files the next durable save has to write.
///
/// Rewriting all of them costs a read of every state file to compare against
/// (`write_atomic_if_changed`) plus a directory sweep, for a workspace where an
/// ordinary edit touches one document. Narrowing that is only safe when the
/// store can *prove* which documents moved, so this defaults to
/// [`CrdtSaveScope::All`] and is narrowed by one route only: an item-level edit,
/// which reports the documents it wrote and cannot add or remove one.
///
/// Getting this wrong does not cost performance, it leaves a document's state
/// stale on disk — and a stale state file is re-seeded from nothing on the next
/// launch. So every route that reaches `self.crdt` any other way (a sync merge,
/// a wholesale rebuild, a structural command, a failed save) widens it back to
/// `All`, and anything added later that forgets to say anything at all keeps
/// whatever the last route set rather than silently narrowing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrdtSaveScope {
    /// Write every document, sweep the ones that went away, retire the legacy
    /// blob. The only scope that can remove a file.
    All,
    /// Write exactly these documents and nothing else.
    Only(HashSet<DocumentId>),
}

impl CrdtSaveScope {
    fn widen_to_all(&mut self) {
        *self = CrdtSaveScope::All;
    }

    fn add(&mut self, documents: impl IntoIterator<Item = DocumentId>) {
        match self {
            // Already writing everything; naming a subset changes nothing.
            CrdtSaveScope::All => {}
            CrdtSaveScope::Only(known) => known.extend(documents),
        }
    }

    /// Nothing to write. A save still runs for the workspace/scheme files.
    pub fn is_empty(&self) -> bool {
        matches!(self, CrdtSaveScope::Only(documents) if documents.is_empty())
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
    // Search/calendar/channel index over `workspace`. Nothing on the hot path
    // reads it, so it is rebuilt lazily (see `index_stale`/`indexed`) rather than
    // on every edit — a full re-tokenize of every item per keystroke otherwise.
    indexed: IndexedWorkspace,
    index_stale: bool,
    dirty: WorkspaceDirtyState,
    replica_id: ReplicaId,
    next_sequence: u64,
    pending_operations: VecDeque<StoreOperation>,
    crdt: WorkspaceCrdtDocuments,
    crdt_save_scope: CrdtSaveScope,
}

impl WorkspaceStore {
    pub fn new<B: AsRef<[u8]>>(
        workspace: Workspace,
        replica_id: ReplicaId,
        initial_dirty: bool,
        crdt_states: HashMap<DocumentId, B>,
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
            index_stale: false,
            dirty,
            replica_id,
            next_sequence: initial_sequence.max(1),
            pending_operations: VecDeque::new(),
            crdt,
            // The first save of a run writes everything: it is what creates the
            // per-document directory, sweeps whatever a previous run left, and
            // retires the legacy blob, so a later incremental save can assume an
            // authoritative directory.
            crdt_save_scope: CrdtSaveScope::All,
        }
    }

    /// Snapshot the long-lived CRDT documents' state for durable persistence and to
    /// seed the background sync's CRDT from this device's latest local edits.
    pub fn crdt_document_states(&self) -> HashMap<DocumentId, Arc<[u8]>> {
        self.crdt.document_states()
    }

    /// The same snapshot as handles that encode on demand, so a caller that is
    /// about to hand the bytes to a background task can do the encoding there.
    pub fn crdt_document_state_handles(&self) -> HashMap<DocumentId, DocumentStateHandle> {
        self.crdt.document_state_handles()
    }

    /// Handles for the documents the next save has to write, and the scope that
    /// describes them — [`CrdtSaveScope::All`] means the save must also sweep
    /// documents that went away, so it cannot be served from a subset.
    ///
    /// Taking the scope resets it: everything recorded from here on belongs to
    /// the *next* save. A save that fails must hand it back with
    /// [`Self::mark_all_crdt_documents_changed`], or the documents it dropped
    /// stay stale on disk.
    pub fn take_crdt_save_scope(
        &mut self,
    ) -> (CrdtSaveScope, HashMap<DocumentId, DocumentStateHandle>) {
        let scope = std::mem::replace(
            &mut self.crdt_save_scope,
            CrdtSaveScope::Only(HashSet::new()),
        );
        let handles = match &scope {
            CrdtSaveScope::All => self.crdt.document_state_handles(),
            CrdtSaveScope::Only(documents) => self.crdt.document_state_handles_for(documents),
        };
        (scope, handles)
    }

    /// Widen the next save back to every document. For any route that changes
    /// the CRDT without being able to name what it touched, and for a save that
    /// failed after taking the scope.
    pub fn mark_all_crdt_documents_changed(&mut self) {
        self.crdt_save_scope.widen_to_all();
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Lazily rebuild the search/calendar/channel index from the current
    /// workspace before returning it. Edits only flag the index stale (cheap), so
    /// the expensive rebuild happens at most once per burst, when a reader
    /// actually asks — and only if something changed since the last read.
    pub fn indexed(&mut self) -> &IndexedWorkspace {
        if self.index_stale {
            self.indexed.replace_workspace(self.workspace.clone());
            self.index_stale = false;
        }
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
                        touched_items: update.touched_items,
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
        // A direct mutation is how a scheme appears without a command (today's
        // Daily Queue), so it can add documents; it is never on the keystroke
        // path, so there is nothing to gain from narrowing it.
        self.crdt_save_scope.widen_to_all();
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
    pub fn replace_workspace_with_crdt_states<B: AsRef<[u8]>>(
        &mut self,
        workspace: Workspace,
        dirty: WorkspaceDirtyState,
        clear_pending_operations: bool,
        crdt_states: HashMap<DocumentId, B>,
    ) {
        let mut workspace = workspace;
        let sync_metadata_dirty = workspace.ensure_sync_metadata();
        let mut dirty = dirty;
        dirty.index |= sync_metadata_dirty;
        self.workspace = workspace;
        self.index_stale = true;
        self.crdt = restored_workspace_crdt(&self.workspace, self.replica_id, &crdt_states);
        // Every document is a fresh object built from bytes that need not match
        // what is on disk, and the workspace may have lost documents whose files
        // must be swept.
        self.crdt_save_scope.widen_to_all();
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
    pub fn merge_sync_crdt_states<B: AsRef<[u8]>>(
        &mut self,
        sync_workspace: &Workspace,
        crdt_states: &HashMap<DocumentId, B>,
    ) -> bool {
        let received_at = Utc::now();
        // `crdt_states` always carries EVERY document, but a sync typically changes a
        // handful. Applying an unchanged document's full state is a costly no-op
        // (decode + integrate the whole document) and doing it for all documents is
        // the dominant cost of landing a sync. Compare each incoming state against
        // this store's current encoding (cheap — the per-document encode cache returns
        // unchanged documents without re-serializing) and apply only what differs.
        let current = self.crdt.document_states();
        let updates = crdt_states
            .iter()
            .filter(|(document, state)| {
                current.get(*document).map(|state| &state[..]) != Some(state.as_ref())
            })
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
                validate_crdt_update_sequence(kind, [state.as_ref()]).ok()?;
                Some(StoredCrdtUpdate {
                    workspace_id: sync_workspace.id,
                    document: *document,
                    kind,
                    replica_id: self.replica_id,
                    sequence: 0,
                    received_at,
                    update_v1: state.as_ref().to_vec(),
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
        self.workspace = outcome.workspace;
        self.index_stale = true;
        self.dirty = WorkspaceDirtyState::all(&self.workspace);
        // A pull can carry an update for any document, and `apply_remote_updates`
        // does not report which ones moved.
        self.crdt_save_scope.widen_to_all();
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
        self.index_stale = true;
        let outcome = self.crdt.sync_changes(&self.workspace, &crdt_changes);
        self.note_crdt_writes(&crdt_changes, &outcome);
        for error in &outcome.errors {
            eprintln!("CRDT sync update failed: {error}");
        }
        outcome.updates
    }

    /// Record which document state files a completed `sync_changes` left stale.
    ///
    /// The emitted updates are exactly the documents it wrote: a document it did
    /// not change produces an empty delta, which `sync_scheme` reports as `None`.
    /// So an item-level edit can name its documents precisely — that is the
    /// keystroke path, and the whole point of narrowing the save.
    ///
    /// Everything else widens back to [`CrdtSaveScope::All`], because only a full
    /// save sweeps the file of a document that went away:
    ///
    ///  - a change set that touches the workspace index is structural, so it can
    ///    create or drop a scheme;
    ///  - `sync_changes` re-emits the whole workspace document when it finds a
    ///    document missing or removed behind the change set's back, which is a
    ///    document-set change by another name — hence the check on what was
    ///    actually emitted rather than on what was asked for;
    ///  - an error means some document's write did not happen, and which one is
    ///    not worth reasoning about at this level.
    fn note_crdt_writes(
        &mut self,
        requested: &WorkspaceCrdtChangeSet,
        outcome: &knotq_sync::WorkspaceCrdtSyncOutcome,
    ) {
        let touched_the_index = outcome
            .updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::PersonalWorkspace);
        if requested.workspace || touched_the_index || !outcome.is_ok() {
            self.crdt_save_scope.widen_to_all();
            return;
        }
        self.crdt_save_scope
            .add(outcome.updates.iter().map(|update| update.document));
    }
}

/// Restore the long-lived CRDT documents from persisted `crdt_states` with a stable,
/// deterministic clientID for this replica. Documents absent from `crdt_states` are
/// left empty and populated by the next sync (adopting the server's canonical
/// identity) or force-emitted as a full snapshot on the next local edit — never
/// rebuilt from plain data with a throwaway identity.
fn restored_workspace_crdt<B: AsRef<[u8]>>(
    workspace: &Workspace,
    replica_id: ReplicaId,
    crdt_states: &HashMap<DocumentId, B>,
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
        | Command::SetItemMarkerFamily { scheme, .. }
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
        | Command::SetItemMarkerFamily { scheme, .. }
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
