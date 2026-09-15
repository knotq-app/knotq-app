use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use knotq_commands::{
    filter_recurring_occurrence_toggles, ChangeSet, Command, CommandOrigin, CommandReceipt,
    WorkspaceCommandExt,
};
use knotq_index::IndexedWorkspace;
use knotq_model::{
    DocumentId, OperationId, ReplicaId, Scheme, SchemeId, SyncDocumentKind, Workspace, WorkspaceId,
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
    // Accumulated but not-yet-reconciled CRDT changes from local/remote edits.
    // `after_workspace_change` merges into this instead of calling
    // `crdt.sync_changes` on every edit — committing a yrs transaction is
    // O(whole document), so per-keystroke reconciliation is the dominant cost of
    // typing on a large scheme. Anything that reads `crdt` for its content (or is
    // about to replace/discard it) must call `flush_crdt` first so no deferred
    // edit is ever silently lost.
    deferred_crdt: WorkspaceCrdtChangeSet,
    // What a scheme held just before the first edit made while its CRDT document
    // had never been populated — a fresh install's starter schemes. The deferred
    // flush populates the document from it before writing the edit, so the edit
    // lands as an edit (`WorkspaceCrdtDocuments::sync_changes_with_bases`).
    population_bases: HashMap<SchemeId, Scheme>,
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
            deferred_crdt: WorkspaceCrdtChangeSet::default(),
            population_bases: HashMap::new(),
        }
    }

    /// Reconcile any deferred CRDT changes (see `deferred_crdt`) into the CRDT
    /// documents now, and record the resulting updates against the most recent
    /// pending operation. A no-op when nothing is deferred.
    ///
    /// The updates are attached to the latest `StoreOperation` because they
    /// represent the document state as of that operation — everything deferred
    /// happened no later than it. If there is no pending operation to attach to
    /// (e.g. only direct/remote edits deferred since the queue was last
    /// drained), a synthetic one is pushed, mirroring `record_direct_crdt_changes`.
    pub fn flush_crdt(&mut self) {
        if self.deferred_crdt.is_empty() {
            return;
        }
        let changes = std::mem::take(&mut self.deferred_crdt);
        // `KNOTQ_TYPING_TIMING=1` reports each flush individually. A profiler
        // aggregates identical stacks, so it cannot tell one 45 ms reconcile
        // apart from 45 one-millisecond ones — and which of those it is decides
        // whether this is a stall worth chasing.
        let started = crdt_flush_timing().then(std::time::Instant::now);
        let bases = std::mem::take(&mut self.population_bases);
        let outcome = self
            .crdt
            .sync_changes_with_bases(&self.workspace, &changes, &bases);
        if let Some(started) = started {
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "[crdt-flush] {elapsed:6.2}ms  ({} scheme(s), {} update(s))",
                changes.schemes.len(),
                outcome.updates.len(),
            );
        }
        // The documents are written HERE now, not when the command was applied,
        // so this is where the save scope has to learn what moved.
        self.note_crdt_writes(&changes, &outcome);
        for error in &outcome.errors {
            eprintln!("CRDT sync update failed: {error}");
        }
        if outcome.updates.is_empty() {
            return;
        }
        if let Some(latest) = self.pending_operations.back_mut() {
            latest.crdt_updates.extend(outcome.updates);
        } else {
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
    }

    /// Snapshot the long-lived CRDT documents' state for durable persistence and to
    /// seed the background sync's CRDT from this device's latest local edits.
    pub fn crdt_document_states(&mut self) -> HashMap<DocumentId, Arc<[u8]>> {
        self.flush_crdt();
        self.crdt.document_states()
    }

    /// The same snapshot as handles that encode on demand, so a caller that is
    /// about to hand the bytes to a background task can do the encoding there.
    pub fn crdt_document_state_handles(&mut self) -> HashMap<DocumentId, DocumentStateHandle> {
        self.flush_crdt();
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
        // Reconcile first: a deferred edit has not reached the documents yet, so
        // both the scope and the handles below would otherwise describe the
        // state from before it and the save would write a stale document.
        self.flush_crdt();
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

    /// Put Daily Queue schemes paged in from disk into the workspace.
    ///
    /// Loading is not an edit: the content is already in the day's CRDT document
    /// and on disk, so nothing is emitted. Only a scheme the index binds as a
    /// day and that is not already present is adopted, so a load can never
    /// overwrite live content. Returns how many were adopted.
    pub fn adopt_loaded_schemes(&mut self, schemes: Vec<Scheme>) -> usize {
        self.flush_crdt();
        let mut adopted = 0;
        for scheme in schemes {
            let bound = self
                .workspace
                .daily_queue
                .values()
                .any(|id| *id == scheme.id);
            if !bound || self.workspace.schemes.contains_key(&scheme.id) {
                continue;
            }
            self.workspace.schemes.insert(scheme.id, scheme);
            adopted += 1;
        }
        if adopted > 0 {
            self.index_stale = true;
        }
        adopted
    }

    /// Rebuild a bound scheme that is missing from the workspace from its CRDT
    /// document — a day whose file was never written or was lost. Returns
    /// whether the scheme is present afterwards.
    ///
    /// This must run before anything creates the scheme afresh: an empty page
    /// written over a document that still holds the day's rows diffs to a
    /// deletion of every row, on every device.
    pub fn materialize_scheme_from_crdt(&mut self, scheme_id: SchemeId) -> bool {
        if self.workspace.schemes.contains_key(&scheme_id) {
            return true;
        }
        self.flush_crdt();
        self.crdt.request_deferred_recovery(scheme_id);
        let materialized = match self
            .crdt
            .materialized_workspace_repair(&self.workspace, &|id| *id == scheme_id)
        {
            Ok(workspace) => workspace,
            Err(err) => {
                eprintln!("materialize scheme {scheme_id} from CRDT failed: {err:#}");
                return false;
            }
        };
        let Some(scheme) = materialized.schemes.get(&scheme_id).cloned() else {
            return false;
        };
        self.workspace.schemes.insert(scheme_id, scheme);
        self.index_stale = true;
        self.dirty.schemes.insert(scheme_id);
        self.crdt_save_scope.widen_to_all();
        true
    }

    /// Normalize the workspace index (dangling archive entries, folder tree
    /// shape) and record the result like any other index edit. Returns whether
    /// anything changed.
    pub fn repair_workspace_index(&mut self) -> bool {
        self.flush_crdt();
        if !self.workspace.normalize_one_level_folders() {
            return false;
        }
        self.workspace.ensure_sync_metadata();
        self.dirty.index = true;
        self.index_stale = true;
        self.deferred_crdt
            .merge(WorkspaceCrdtChangeSet::default().workspace());
        self.flush_crdt();
        true
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

    /// How much unsynced local work there is, WITHOUT reconciling anything.
    ///
    /// The sync indicator only needs to know whether there is unsynced work and
    /// roughly how much, and the title bar renders it on every frame — flushing
    /// there would reconcile once per frame and defeat the deferral entirely.
    /// Deferred changes count as one edit: they become one or more on the next
    /// flush, and the indicator only distinguishes zero from non-zero.
    pub fn unsynced_edit_count(&self) -> usize {
        let queued: usize = self
            .pending_operations
            .iter()
            .map(|operation| operation.crdt_updates.len())
            .sum();
        queued + usize::from(!self.deferred_crdt.is_empty())
    }

    pub fn has_pending_crdt_edits(&mut self) -> bool {
        self.flush_crdt();
        self.pending_operations
            .iter()
            .any(|op| !op.crdt_updates.is_empty())
    }

    pub fn pending_crdt_edits(&mut self) -> Vec<PendingCrdtEdit> {
        self.flush_crdt();
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
        // Reconcile any deferred local/remote edits into `self.crdt` before it is
        // snapshotted below — otherwise the snapshot (and the rebuilt CRDT it
        // seeds) would silently omit whatever hadn't been flushed yet.
        self.flush_crdt();
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
        // Defensive: reconcile anything deferred against the OLD workspace/crdt
        // before either is replaced below. `replace_workspace` already flushes
        // before calling this (so this is normally a no-op there), but this is
        // also reachable directly (see `AppState::replace_workspace_from_sync`'s
        // fallback), and `self.crdt` is about to be discarded wholesale.
        self.flush_crdt();
        let mut workspace = workspace;
        let sync_metadata_dirty = workspace.ensure_sync_metadata();
        let mut dirty = dirty;
        dirty.index |= sync_metadata_dirty;
        self.workspace = workspace;
        self.index_stale = true;
        self.crdt = restored_workspace_crdt(&self.workspace, self.replica_id, &crdt_states);
        self.population_bases.clear();
        // Every document is a fresh object built from bytes that need not match
        // what is on disk, and the workspace may have lost documents whose files
        // must be swept.
        self.crdt_save_scope.widen_to_all();
        self.dirty = dirty;
        if clear_pending_operations {
            self.pending_operations.clear();
        }
    }

    /// The replace fallback for landing a sync run whose result could not be
    /// merged into the live documents — the run re-identified the workspace
    /// document (first sync after signing in), or the live documents were never
    /// seeded (a fresh install). Adopts the run's documents wholesale, then
    /// re-applies every still-unpushed local edit on top.
    ///
    /// Those edits stay queued and reach the server regardless, so showing them
    /// now is exactly what this device converges to. Dropping them here made an
    /// edit applied while the run was in flight vanish from the screen until a
    /// later round trip. An edit to the workspace document authored under the
    /// pre-sign-in document id is re-addressed to the new id, so it is pushed
    /// to the account's workspace document rather than a stray one.
    pub fn replace_from_sync<B: AsRef<[u8]>>(
        &mut self,
        workspace: Workspace,
        crdt_states: HashMap<DocumentId, B>,
    ) {
        self.flush_crdt();
        let previous_workspace_document = self.workspace.sync.id;
        let dirty = WorkspaceDirtyState::all(&workspace);
        self.replace_workspace_with_crdt_states(workspace, dirty, false, crdt_states);
        let current_workspace_document = self.workspace.sync.id;
        self.remap_pending_workspace_document(
            previous_workspace_document,
            current_workspace_document,
        );
        let received_at = Utc::now();
        let updates: Vec<StoredCrdtUpdate> = self
            .pending_operations
            .iter()
            .flat_map(|operation| operation.crdt_updates.iter())
            .map(|update| StoredCrdtUpdate {
                workspace_id: self.workspace.id,
                document: update.document,
                kind: update.kind,
                replica_id: self.replica_id,
                sequence: 0,
                received_at,
                update_v1: update.update_v1.clone(),
            })
            .collect();
        if updates.is_empty() {
            return;
        }
        let outcome = self.crdt.apply_remote_updates(&self.workspace, &updates);
        for error in &outcome.workspace_errors {
            eprintln!(
                "re-applying unpushed edits after a sync replace: {}",
                error.message
            );
        }
        for error in &outcome.document_errors {
            if !error.unknown_scheme_document {
                eprintln!(
                    "re-applying unpushed edits after a sync replace: {}",
                    error.message
                );
            }
        }
        if outcome.workspace_is_ok() {
            self.workspace = outcome.workspace;
            self.index_stale = true;
            self.dirty = WorkspaceDirtyState::all(&self.workspace);
            self.crdt_save_scope.widen_to_all();
        }
        self.reroot_pre_sign_in_edits();
    }

    /// Edits authored before signing in still name the pre-sign-in root folder,
    /// so merging or re-applying them brings that folder back as an ordinary
    /// (visible) one beside the account's root. Re-root them exactly as the sync
    /// run canonicalized its own snapshot, and record the repair like any index
    /// edit so the server is re-rooted too.
    fn reroot_pre_sign_in_edits(&mut self) {
        let workspace_id = self.workspace.id;
        let (repair_needed, _) = self
            .workspace
            .canonicalize_personal_sync_identity_with_change(workspace_id);
        if repair_needed {
            self.dirty.index = true;
            self.index_stale = true;
            self.deferred_crdt
                .merge(WorkspaceCrdtChangeSet::default().workspace());
            self.flush_crdt();
            self.crdt_save_scope.widen_to_all();
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
        // The comparison below and `apply_remote_updates` both read/mutate
        // `self.crdt` directly; anything deferred must land there first or it is
        // lost the moment `self.workspace` is overwritten with the merge result.
        self.flush_crdt();
        // The run adopted the account's canonical workspace identity (first sign-in,
        // or an account switch) while this store still holds the pre-sign-in one.
        // Its workspace document then never matches ours: the merge would skip the
        // index update as a "document id mismatch" and refuse the rest, and landing
        // would fall back to replacing the workspace — which half-applies an edit
        // made while the run was in flight (a line moved between schemes came back
        // in both). Re-key our document first, exactly as the run re-keyed its own.
        if sync_workspace.sync.id != self.workspace.sync.id
            && !self.adopt_sync_workspace_identity(sync_workspace)
        {
            return false;
        }
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

    /// Move this store onto `sync_workspace`'s canonical identity: the workspace
    /// document is re-keyed with its content (and history) intact, and unpushed
    /// edits addressed to the old document follow it. Returns `false` when the
    /// identities cannot be reconciled, so the caller falls back to a replace.
    fn adopt_sync_workspace_identity(&mut self, sync_workspace: &Workspace) -> bool {
        let previous_document = self.workspace.sync.id;
        let mut workspace = self.workspace.clone();
        workspace.canonicalize_personal_sync_identity_with_change(sync_workspace.id);
        workspace.ensure_sync_metadata();
        if workspace.sync.id != sync_workspace.sync.id {
            return false;
        }
        if let Err(err) = self
            .crdt
            .reidentify_workspace_document(sync_workspace.sync.id)
        {
            eprintln!("sync merge: re-identify workspace document: {err:#}");
            return false;
        }
        self.workspace = workspace;
        self.remap_pending_workspace_document(previous_document, sync_workspace.sync.id);
        self.dirty.index = true;
        self.index_stale = true;
        true
    }

    /// Point unpushed edits addressed to workspace document `from` at `to`.
    fn remap_pending_workspace_document(&mut self, from: DocumentId, to: DocumentId) {
        if from == to {
            return;
        }
        for operation in &mut self.pending_operations {
            for update in &mut operation.crdt_updates {
                if update.document == from {
                    update.document = to;
                }
            }
        }
    }

    /// Remember what each scheme `command` writes holds right now, for schemes
    /// whose CRDT document has never been populated (see `population_bases`).
    fn record_population_bases(&mut self, command: &Command) {
        for scheme_id in command.crdt_documents().schemes {
            if self.population_bases.contains_key(&scheme_id)
                || !self.crdt.scheme_document_is_unpopulated(scheme_id)
            {
                continue;
            }
            if let Some(scheme) = self.workspace.schemes.get(&scheme_id) {
                self.population_bases.insert(scheme_id, scheme.clone());
            }
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
        let may_change_document_set = command_may_change_document_set(&command);
        self.record_population_bases(&command);
        let receipt = self.workspace.apply(command.clone())?;
        let crdt_changes = crdt_change_set_for_command(&command);
        let crdt_updates =
            self.after_workspace_change(&receipt.touched, crdt_changes, may_change_document_set);
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
        let may_change_document_set = command_may_change_document_set(&command);
        self.record_population_bases(&command);
        let receipt = self.workspace.apply(command)?;
        self.after_workspace_change(&receipt.touched, crdt_changes, may_change_document_set);
        Ok(Some(receipt))
    }

    fn after_workspace_change(
        &mut self,
        changeset: &ChangeSet,
        mut crdt_changes: WorkspaceCrdtChangeSet,
        may_change_document_set: bool,
    ) -> Vec<CrdtDocumentUpdate> {
        for scheme_id in &changeset.schemes {
            self.dirty.schemes.insert(*scheme_id);
        }
        self.dirty.index = true;
        // `ensure_sync_metadata` is O(total schemes + daily-queue entries): it
        // allocates a HashSet of every daily-queue scheme id and walks every
        // `scheme_sync`/`folder_sync` entry. On a real workspace (hundreds of
        // documents) that cost lands on EVERY applied command, including a
        // single keystroke, and scales with total workspace size rather than
        // with the edit. A command that cannot add, remove, or otherwise
        // rebind a scheme or folder cannot invalidate sync metadata, so the
        // caller only asks for the check (via `command_may_change_document_set`)
        // when it might have. See that function for the exhaustive command
        // classification.
        if may_change_document_set && self.workspace.ensure_sync_metadata() {
            self.dirty.index = true;
            crdt_changes.workspace = true;
        }
        self.index_stale = true;
        // Defer the actual CRDT reconciliation (a yrs commit, O(whole document))
        // instead of doing it on every edit. A keystroke burst then reconciles
        // once, on the next flush, rather than once per character. Nothing is
        // lost: every reader/replacer of `self.crdt` calls `flush_crdt` first,
        // and the save scope is recorded there too.
        self.deferred_crdt.merge(crdt_changes);
        Vec::new()
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
    let documents = command.crdt_documents();
    WorkspaceCrdtChangeSet {
        workspace: documents.workspace,
        schemes: documents.schemes.into_iter().collect(),
    }
}

/// Whether `command` could have changed *which* schemes or folders exist (or
/// which document a scheme/folder is bound to) — i.e. whether it could have
/// invalidated `Workspace::sync_metadata_is_current`.
///
/// `ensure_sync_metadata` repairs `scheme_sync`/`folder_sync` bindings so every
/// live scheme and folder (plus every daily-queue scheme) has a well-formed
/// sync document binding, and nothing stale is left over. Its fast path
/// (`sync_metadata_is_current`) is O(total schemes + daily-queue entries), so
/// running it after every command — including a single keystroke — costs
/// proportional to the whole workspace, not to the edit. A command whose
/// effects are confined to fields *within* an already-existing scheme/folder
/// (item text, item structure, scheme/folder metadata like name/color/expanded)
/// cannot add, remove, or rebind a scheme or folder, so it cannot invalidate
/// those bindings and the repair pass can be skipped.
///
/// DELIBERATELY CONSERVATIVE: this match has NO catch-all arm. Every `Command`
/// variant is listed explicitly, so a new variant fails to COMPILE here rather
/// than silently defaulting to "skip the check" (which would let sync metadata
/// drift out of repair and corrupt sync). If you add a `Command` variant and
/// the compiler sends you here, default to `true` unless you can prove the
/// variant never touches `workspace.schemes`, `workspace.folders`,
/// `workspace.daily_queue`, or any `scheme_sync`/`folder_sync` entry's
/// identity.
fn command_may_change_document_set(command: &Command) -> bool {
    match command {
        // Mint/remove/rebind a folder, or otherwise change which folders exist
        // or how they're archived. All of these touch `workspace.folders`
        // and/or `folder_sync`.
        Command::CreateFolder { .. }
        | Command::RestoreFolder { .. }
        | Command::RestoreDeletedFolder { .. }
        | Command::DeleteFolder { .. }
        | Command::PermanentlyDeleteFolder { .. } => true,

        // Mint/remove/rebind a scheme, or otherwise change which schemes
        // exist. All of these touch `workspace.schemes` and/or `scheme_sync`
        // (directly, or via `recently_deleted`/archive bookkeeping that
        // `ensure_sync_metadata` also normalizes against).
        Command::CreateScheme { .. }
        | Command::RestoreScheme { .. }
        | Command::RestoreDeletedScheme { .. }
        | Command::DeleteScheme { .. }
        | Command::PermanentlyDeleteScheme { .. } => true,

        // Moves a scheme or folder between folders. Does not itself add or
        // remove a scheme/folder id, but it can move a node into or out of an
        // archived (deleted) folder's subtree, which changes deletion/archive
        // state that `ensure_sync_metadata`'s daily-queue and stale-binding
        // checks reason about. Kept conservative: true.
        Command::MoveNode { .. } => true,

        // Binds a day in `daily_queue` and may create its scheme.
        Command::EnsureDailyQueue { .. } => true,

        // Folder/scheme metadata-only edits: rename, recolor, toggle
        // google-calendar sync, change source, toggle expanded. Verified
        // against `desktop/commands/src/apply/folder.rs` and
        // `.../apply/scheme.rs`: each of these looks up the existing
        // folder/scheme by id and mutates exactly one field in place. None of
        // them touch `workspace.folders`, `workspace.schemes`,
        // `workspace.daily_queue`, `folder_sync`, or `scheme_sync` keys.
        Command::RenameFolder { .. }
        | Command::SetFolderExpanded { .. }
        | Command::RenameScheme { .. }
        | Command::SetSchemeColor { .. }
        | Command::SetSchemeGsync { .. }
        | Command::SetSchemeSource { .. } => false,

        // Item-content/structure edits, all scoped to items inside an
        // already-existing scheme (looked up by `scheme` id in
        // `desktop/commands/src/apply/item.rs`). None of these create,
        // delete, or rebind a scheme or folder.
        Command::InsertItem { .. }
        | Command::UpdateItemText { .. }
        | Command::ReplaceItem { .. }
        | Command::SetItemIndent { .. }
        | Command::SetItemMarker { .. }
        | Command::SetItemMarkerFamily { .. }
        | Command::SetItemDate { .. }
        | Command::SetItemRecurrence { .. }
        | Command::SetItemPriority { .. }
        | Command::SetOccurrenceNotificationOffset { .. }
        | Command::ToggleOccurrence { .. }
        | Command::DeleteItem { .. }
        | Command::ReorderItem { .. } => false,

        Command::Batch(commands) => commands.iter().any(command_may_change_document_set),
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
        Command::EnsureDailyQueue { date } => {
            out.insert(knotq_model::daily_queue_scheme_id(*date));
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

/// Whether `KNOTQ_TYPING_TIMING=1` asked for per-flush reconcile timings.
///
/// A sampling profiler aggregates identical stacks, so it cannot tell one 45 ms
/// reconcile apart from forty-five 1 ms ones — and which of those it is decides
/// whether a deferred flush is a user-visible stall or just steady overhead.
fn crdt_flush_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("KNOTQ_TYPING_TIMING").is_ok_and(|value| value != "0" && !value.is_empty())
    })
}
