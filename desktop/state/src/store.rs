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
    /// `next_sequence` when `deferred_crdt` last went from empty to non-empty:
    /// the oldest operation its updates may be attached to (see `flush_crdt`).
    deferred_since: u64,
    // What a scheme held just before the first edit made while its CRDT document
    // had never been populated — a fresh install's starter schemes. The deferred
    // flush populates the document from it before writing the edit, so the edit
    // lands as an edit (`WorkspaceCrdtDocuments::sync_changes_with_bases`).
    population_bases: HashMap<SchemeId, Scheme>,
    // Same idea one level up: what the whole workspace held just before the
    // first edit made while the workspace-index CRDT document had never been
    // populated (either at construction — a fresh install's very first save —
    // or from a later edit). Without this, that edit's index write competes
    // on equal footing (clientID alone) against the account's own from-scratch
    // population arriving from another device, and can silently lose — see
    // `an_edit_made_while_a_sync_is_in_flight_is_pushed`. Read (never taken)
    // by `flush_crdt`; the sole consumer that takes it is
    // `merge_sync_crdt_states`, which must capture it before ANY flush this
    // landing triggers (including ones earlier than its own, e.g.
    // `drop_unbound_pending_crdt_edits`) — see its call site.
    workspace_population_base: Option<Workspace>,
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
        // A device with no saved CRDT state (a genuinely fresh install, or one
        // that never reached its first successful save) constructs an
        // unpopulated workspace document here. `dirty=all` below means the
        // very first save flushes it before any edit command — let alone one
        // made while a sync is in flight — ever gets a chance to record a
        // population base for it. Capture the base NOW, so if this device
        // later adopts an account's canonical identity (first sign-in), that
        // adoption can still rebuild this document deterministically instead
        // of being stuck with whatever clientID the first save used.
        let workspace_population_base = crdt
            .workspace_document_is_unpopulated()
            .then(|| workspace.clone());
        // A crash between the plain workspace save and the CRDT save leaves
        // every scheme's content only in the plain files too. Preserve those
        // pre-sign-in bases at construction so first-sync canonicalization can
        // re-root them just like the workspace index, instead of adopting the
        // account's content and silently discarding the offline workspace.
        let population_bases = workspace
            .schemes
            .iter()
            .filter_map(|(scheme_id, scheme)| {
                crdt.scheme_document_is_unpopulated(*scheme_id)
                    .then(|| (*scheme_id, scheme.clone()))
            })
            .collect();
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
            deferred_since: 0,
            population_bases,
            workspace_population_base,
        }
    }

    /// Restore the durable CRDT queue into the live store before the first save
    /// of a relaunched session. The queue is deliberately not materialized as
    /// commands: the plain workspace and CRDT states already contain its
    /// content, while these updates still have to be carried forward until a
    /// sync acknowledges them. Keeping them as synthetic operations gives the
    /// normal save path an accurate complete snapshot, so reconciling the
    /// on-disk queue with the live store cannot discard an edit that was
    /// persisted by the previous session but has not reached the server.
    pub fn restore_pending_crdt_edits(
        &mut self,
        pending: impl IntoIterator<Item = PendingCrdtEdit>,
    ) {
        for edit in pending {
            let update = edit.as_update();
            if let Some(operation) = self.pending_operations.iter_mut().find(|operation| {
                operation.id == edit.operation_id && operation.sequence == edit.local_sequence
            }) {
                operation.crdt_updates.push(update);
                continue;
            }
            self.pending_operations.push_back(StoreOperation {
                id: edit.operation_id,
                workspace_id: edit.workspace_id,
                replica_id: edit.replica_id,
                sequence: edit.local_sequence,
                origin: CommandOrigin::User,
                created_at: edit.created_at,
                command: Command::Batch(Vec::new()),
                crdt_updates: vec![update],
            });
            self.next_sequence = self
                .next_sequence
                .max(edit.local_sequence.saturating_add(1));
        }
    }

    /// Recover edits that reached the plain workspace files before a paired
    /// CRDT save completed.
    ///
    /// The recovery base is written before the workspace file is replaced. A
    /// relaunch therefore has enough information to turn the plain-file delta
    /// back into the same CRDT operation the normal command path would have
    /// produced. This is especially important for a first-sync install: an
    /// edited daily page must be queued as a local delta, not mistaken for
    /// untouched starter content when the account's existing page is pulled.
    pub fn recover_workspace_save(&mut self, base: Workspace) {
        self.flush_crdt();
        let current = self.workspace.clone();
        let mut changes = WorkspaceCrdtChangeSet::default();

        if self.crdt.workspace_document_is_unpopulated() {
            if self.crdt.workspace_document_differs(&base, &current) {
                self.workspace_population_base = Some(base.clone());
                changes.workspace = true;
            }
        } else if self.crdt.workspace_document_differs(&base, &current) {
            changes.workspace = true;
        }

        for (scheme_id, scheme) in &current.schemes {
            let changed = base.schemes.get(scheme_id) != Some(scheme);
            if !changed {
                continue;
            }
            if self.crdt.scheme_document_is_unpopulated(*scheme_id) {
                if let Some(previous) = base.schemes.get(scheme_id) {
                    self.population_bases.insert(*scheme_id, previous.clone());
                }
            }
            changes.schemes.insert(*scheme_id);
        }

        if changes.is_empty() {
            return;
        }
        self.defer_crdt(changes);
        self.flush_crdt();
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
        // NOT taken (unlike `bases` above): a scheme's population base can
        // only ever be consumed here, but the workspace's may still be needed
        // by `merge_sync_crdt_states`/`adopt_sync_workspace_identity` LATER in
        // the SAME landing — pending cleanup (`drop_unbound_pending_crdt_edits`)
        // flushes before that runs. Populating from it here is safe to repeat:
        // once populated, `workspace_document_is_unpopulated` is false, so an
        // unconsumed base sitting here across several flushes just means it is
        // offered — and ignored — every time until something actually takes
        // it (only `merge_sync_crdt_states` does).
        let outcome = self.crdt.sync_changes_with_bases_and_workspace_base(
            &self.workspace,
            &changes,
            &bases,
            self.workspace_population_base.as_ref(),
        );
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
        // Only an operation made after the deferral began may carry these
        // updates. An older one can predate a sync run's snapshot, and landing
        // that run clears it as pushed — taking these unsent updates with it.
        let deferred_since = self.deferred_since;
        if let Some(latest) = self
            .pending_operations
            .back_mut()
            .filter(|latest| latest.sequence >= deferred_since)
        {
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

    /// Rebuild the visible item placement after a sync landing has authored
    /// local repair commands. Those commands update the CRDT documents, but
    /// they must not create a second materialization path that can leave one
    /// item visible in two schemes. The CRDT projection owns the one-item/
    /// one-placement rule, so this is the single reconciliation boundary.
    pub fn reconcile_item_placements(&mut self) -> bool {
        self.flush_crdt();
        let Ok(workspace) = self
            .crdt
            .materialized_workspace_repair(&self.workspace, &|_| false)
        else {
            return false;
        };
        let mut changed_schemes = HashSet::new();
        for (scheme_id, projected) in workspace.schemes {
            let Some(current) = self.workspace.schemes.get_mut(&scheme_id) else {
                continue;
            };
            if current.items != projected.items {
                current.items = projected.items;
                changed_schemes.insert(scheme_id);
            }
        }
        if changed_schemes.is_empty() {
            return false;
        }
        self.index_stale = true;
        self.dirty.schemes.extend(changed_schemes);
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
        self.defer_crdt(WorkspaceCrdtChangeSet::default().workspace());
        self.flush_crdt();
        true
    }

    /// Queue CRDT reconciliation for the next `flush_crdt`, remembering which
    /// operations the result may be attached to.
    fn defer_crdt(&mut self, changes: WorkspaceCrdtChangeSet) {
        if self.deferred_crdt.is_empty() {
            self.deferred_since = self.next_sequence;
        }
        self.deferred_crdt.merge(changes);
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
        snapshot_watermark: u64,
    ) -> usize {
        let mut cleared = 0;
        for operation in &mut self.pending_operations {
            // Only an operation the run's snapshot held can have been pushed. The
            // run queues edits of its own under sequences past the store's, so an
            // edit made while it was in flight can share or undercut a pushed
            // sequence without ever having been sent.
            if operation.sequence > through_local_sequence
                || operation.sequence >= snapshot_watermark
            {
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

    /// Clear exactly the local edits acknowledged by a completed sync run.
    /// The sequence watermark remains a fallback for older callers, but exact
    /// operation identities are required when a landing created an operation
    /// at the same sequence boundary as the run snapshot.
    pub fn clear_pushed_crdt_edits_exact(
        &mut self,
        document: DocumentId,
        sent_edits: &[(OperationId, u64)],
    ) -> usize {
        if sent_edits.is_empty() {
            return 0;
        }
        let sent: std::collections::HashSet<(OperationId, u64)> =
            sent_edits.iter().copied().collect();
        let mut cleared = 0;
        for operation in &mut self.pending_operations {
            let before = operation.crdt_updates.len();
            operation.crdt_updates.retain(|update| {
                !(update.document == document && sent.contains(&(operation.id, operation.sequence)))
            });
            cleared += before - operation.crdt_updates.len();
        }
        self.pending_operations
            .retain(|operation| !operation.crdt_updates.is_empty());
        cleared
    }

    /// Drop unpushed edits addressed to a document this workspace no longer binds
    /// (a scheme permanently deleted after it was edited), returning how many.
    ///
    /// A sync run discards such an edit before pushing — the server has no base
    /// for the document and never will — but it only discards its own copy. The
    /// store's copy was handed to the next run, discarded again, and so on for
    /// ever: the device reported unsynced work it could never push.
    pub fn drop_unbound_pending_crdt_edits(&mut self) -> usize {
        self.flush_crdt();
        let workspace = &self.workspace;
        let bound = |document: DocumentId| {
            workspace.sync.id == document
                || workspace
                    .scheme_sync
                    .values()
                    .any(|meta| meta.id == document)
                || workspace
                    .folder_sync
                    .values()
                    .any(|meta| meta.id == document)
        };
        let mut dropped = 0;
        for operation in &mut self.pending_operations {
            let before = operation.crdt_updates.len();
            operation
                .crdt_updates
                .retain(|update| bound(update.document));
            dropped += before - operation.crdt_updates.len();
        }
        if dropped > 0 {
            self.pending_operations
                .retain(|operation| !operation.crdt_updates.is_empty());
        }
        dropped
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
            ..WorkspaceCrdtChangeSet::default()
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
        // The replacement states are already the canonical sync result. A
        // pre-sign-in population base must not survive the replacement or the
        // next landing will treat the now-canonical document as another
        // first-sync adoption and enqueue a duplicate full snapshot forever.
        self.workspace_population_base = None;
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
            self.defer_crdt(WorkspaceCrdtChangeSet::default().workspace());
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
        // A crash can leave plain scheme files ahead of an entirely unseeded
        // CRDT with no pending operation to trigger `flush_crdt`. Re-express
        // those schemes now, before applying the account's state, so their
        // deterministic population and offline edits merge into the remote
        // document instead of being discarded by first-sync replacement.
        if !self.population_bases.is_empty() {
            let bases = std::mem::take(&mut self.population_bases);
            // `population_bases` also contains untouched starter schemes on a
            // fresh install. Re-expressing those schemes while adopting an
            // account is wrong: the account's remote document is authoritative
            // for content this device has never edited, and a full snapshot of
            // the starter copy can resurrect lines the account deleted. A
            // scheme whose plain copy differs from its captured base, however,
            // has a real local edit (including an edit made while the first
            // sync was in flight) and must still be re-expressed before the
            // remote state is merged.
            let changed_bases: HashMap<_, _> = bases
                .into_iter()
                .filter(|(scheme_id, base)| {
                    self.workspace
                        .schemes
                        .get(scheme_id)
                        .is_some_and(|scheme| scheme != base)
                })
                .collect();
            let schemes = changed_bases.keys().copied().collect();
            let outcome = self.crdt.sync_changes_with_bases(
                &self.workspace,
                &WorkspaceCrdtChangeSet {
                    workspace: false,
                    schemes,
                    ..WorkspaceCrdtChangeSet::default()
                },
                &changed_bases,
            );
            if !outcome.updates.is_empty() {
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
            for error in outcome.errors {
                eprintln!("sync merge: re-root scheme document: {error}");
            }
        }
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
        // The run's index can bind a scheme this store has loaded to a content
        // document the store has never held — another device re-created the
        // scheme's document. Merging only applies updates to documents the store
        // already has, so that document would never be built here: the scheme
        // would keep materializing from its old plain copy while every sync
        // reported a change (deep production fuzz, seed 10004: lines deleted on
        // every other device stayed on one device forever). Let the caller replace
        // instead; that rebuilds the documents from the run's states.
        let known_documents = self.crdt.known_document_ids();
        if sync_workspace.scheme_sync.iter().any(|(scheme, meta)| {
            meta.kind == SyncDocumentKind::Scheme
                && self.workspace.schemes.contains_key(scheme)
                && !known_documents.contains(&meta.id)
                && crdt_states.contains_key(&meta.id)
        }) {
            return false;
        }
        let received_at = Utc::now();
        // `flush_crdt` above integrates edits made while the background run
        // was in flight, but the run's full document states were captured
        // before those edits existed. Applying those states afterwards can
        // therefore leave a locally re-added item tombstoned again (notably a
        // Daily Queue carry-over: the plain workspace still shows the move,
        // while the target CRDT document remains deleted). Keep the exact
        // queued deltas and replay them after the run state below. This is a
        // causal replay of the already-authored operations, not a fresh
        // whole-workspace rewrite, so unrelated remote fields are untouched.
        let in_flight_updates: Vec<StoredCrdtUpdate> = self
            .pending_operations
            .iter()
            .flat_map(|operation| {
                operation
                    .crdt_updates
                    .iter()
                    .map(|update| StoredCrdtUpdate {
                        workspace_id: self.workspace.id,
                        document: update.document,
                        kind: update.kind,
                        replica_id: self.replica_id,
                        sequence: operation.sequence,
                        received_at,
                        update_v1: update.update_v1.clone(),
                    })
            })
            .collect();
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
        // Route the full CRDT states against the sync result's canonical index,
        // not the stale UI index. The background snapshot includes loaded
        // on-disk Daily pages that may be absent from `self.workspace` after a
        // lazy reload; using the latter makes their valid content look like an
        // orphan and drops it during landing. In-flight local edits are replayed
        // below, so the canonical index here does not discard them.
        let mut outcome = self.crdt.apply_remote_updates(sync_workspace, &updates);
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
        if mergeable && !in_flight_updates.is_empty() {
            let replayed = self
                .crdt
                .apply_remote_updates(&outcome.workspace, &in_flight_updates);
            for error in &replayed.workspace_errors {
                eprintln!("sync merge local replay workspace error: {}", error.message);
            }
            for error in &replayed.document_errors {
                if !error.unknown_scheme_document {
                    eprintln!("sync merge local replay document error: {}", error.message);
                    mergeable = false;
                }
            }
            if replayed.workspace_is_ok()
                && replayed
                    .document_errors
                    .iter()
                    .all(|error| error.unknown_scheme_document)
            {
                outcome = replayed;
            } else {
                mergeable = false;
            }
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
        // A mismatched identity can mean two very different things: this
        // device's own content is still waiting to settle onto an account it
        // is genuinely joining (its schemes are the account's schemes, just
        // unkeyed), or this device already had a DIFFERENT account's content
        // loaded and is now switching accounts entirely (chaos-fuzz account
        // switches; also possible if a prior landing's identity adoption
        // never persisted). Re-keying or repopulating in the second case
        // would carry the old account's schemes forward under the new
        // account's id — or, worse, push them as a full snapshot that
        // clobbers the new account's real content server-side. Guard on
        // disjointness rather than trying to name "which account": if this
        // device already holds schemes and NONE of them are schemes the
        // incoming account knows about, this is not an unsettled identity,
        // it is unrelated content — refuse so the caller falls back to
        // `replace_workspace_from_sync`, which is built to safely adopt
        // wholesale-different content instead of merging it.
        if !self.workspace.schemes.is_empty()
            && self
                .workspace
                .schemes
                .keys()
                .all(|id| !sync_workspace.schemes.contains_key(id))
        {
            return false;
        }
        // A document populated from `workspace_population_base` (this
        // replica's content before it ever adopted an account identity) is
        // hashed under this replica's own pre-canonical `sync.id` — a plain
        // re-key can't fix that, since it only rebinds the document without
        // touching the wrong-hashed population inside. Rebuild the population
        // under the now-known canonical identity instead, keeping the edit
        // made on top of it. See `repopulate_workspace_canonically`.
        let mut index_changed = false;
        let repopulated = if let Some(base) = self.workspace_population_base.as_ref() {
            let mut canonical_base = base.clone();
            canonical_base.canonicalize_personal_sync_identity_with_change(sync_workspace.id);
            canonical_base.ensure_sync_metadata();
            // What matters for the outgoing snapshot is whether THIS device
            // made a real edit on top of its own pre-sync base — not whether
            // its (not-yet-merged) content differs from the account's. A
            // device that simply hasn't merged the account's state yet always
            // differs from `sync_workspace` in that direction (the account
            // knows about content this device has never seen, e.g. another
            // device's scheme), and pushing THIS device's narrower content as
            // a full snapshot would overwrite that content server-side —
            // exactly the scheme-loss bug `0a` fixed a different path into.
            index_changed = self
                .crdt
                .workspace_document_differs(&canonical_base, &workspace);
            if let Err(err) = self.crdt.repopulate_workspace_canonically(
                &canonical_base,
                &workspace,
                sync_workspace.sync.id,
            ) {
                eprintln!("sync merge: repopulate workspace document canonically: {err:#}");
                return false;
            }
            self.workspace_population_base = None;
            true
        } else {
            if let Err(err) = self
                .crdt
                .reidentify_workspace_document(sync_workspace.sync.id)
            {
                eprintln!("sync merge: re-identify workspace document: {err:#}");
                return false;
            }
            false
        };
        self.workspace = workspace;
        if repopulated {
            // The repopulation above already folds any pre-canonical edit into
            // the new canonical document (see `repopulate_workspace_canonically`).
            // Any pending push queued for the OLD document before this landing
            // was captured from that now-replaced document and shares no causal
            // history with the fresh one — relabeling its `document` field the
            // way `remap_pending_workspace_document` does for a plain re-key
            // would push stale, foreign-clientID bytes under the canonical id
            // instead of the edit the repopulation already carried forward.
            // Drop it.
            self.drop_pending_workspace_document_updates(previous_document);
            // `fresh` (built inside `repopulate_workspace_canonically`) already
            // holds the population AND the edit as of its own construction, so
            // there is no "before" left within it for an incremental diff to
            // find — the flush below's `sync_snapshot` correctly sees no change
            // relative to what `fresh` already has and pushes nothing. Queue the
            // document's full state directly instead, the same way a first-ever
            // bootstrap does: it merges into any base (the server's old
            // population, another device's) by shared clientID/clock, so the
            // edit still lands even though it is sent as a whole snapshot
            // rather than a delta.
            let full = self
                .crdt
                .full_snapshot_updates_for_documents(&HashSet::from([sync_workspace.sync.id]));
            if index_changed && !full.updates.is_empty() {
                self.pending_operations.push_back(StoreOperation {
                    id: OperationId::new(),
                    workspace_id: self.workspace.id,
                    replica_id: self.replica_id,
                    sequence: self.next_sequence,
                    origin: CommandOrigin::User,
                    created_at: Utc::now(),
                    command: Command::Batch(Vec::new()),
                    crdt_updates: full.updates,
                });
                self.next_sequence += 1;
            }
        } else {
            self.remap_pending_workspace_document(previous_document, sync_workspace.sync.id);
        }
        self.dirty.index = true;
        self.index_stale = true;
        // `reidentify_workspace_document` only re-keys the document's external
        // binding; the `sync` metadata stored in its own "meta" map content (read
        // back by every future materialization, including the one later in this
        // same merge) still names the old identity. Left deferred, that stale
        // content re-materializes over `self.workspace` before anything flushes
        // it — reconcile it into the re-keyed document immediately instead of
        // trusting a future flush to catch up in time.
        self.defer_crdt(WorkspaceCrdtChangeSet::default().workspace());
        self.flush_crdt();
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

    /// Discard any unpushed edits addressed to workspace document `document` —
    /// used instead of [`Self::remap_pending_workspace_document`] after
    /// [`WorkspaceCrdtDocuments::repopulate_workspace_canonically`], whose
    /// output already carries forward whatever those edits contained. See the
    /// call site in `adopt_sync_workspace_identity`.
    fn drop_pending_workspace_document_updates(&mut self, document: DocumentId) {
        for operation in &mut self.pending_operations {
            operation
                .crdt_updates
                .retain(|update| update.document != document);
        }
    }

    /// Remember what each scheme `command` writes holds right now, for schemes
    /// whose CRDT document has never been populated (see `population_bases`).
    fn record_population_bases(&mut self, command: &Command) {
        let documents = command.crdt_documents();
        for scheme_id in documents.schemes {
            if self.population_bases.contains_key(&scheme_id)
                || !self.crdt.scheme_document_is_unpopulated(scheme_id)
            {
                continue;
            }
            if let Some(scheme) = self.workspace.schemes.get(&scheme_id) {
                self.population_bases.insert(scheme_id, scheme.clone());
            }
        }
        if documents.workspace
            && self.workspace_population_base.is_none()
            && self.crdt.workspace_document_is_unpopulated()
        {
            self.workspace_population_base = Some(self.workspace.clone());
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
        self.defer_crdt(crdt_changes);
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
    let mut deleted_items: HashMap<SchemeId, HashSet<String>> = HashMap::new();
    collect_deleted_item_ids(command, &mut deleted_items);
    // A batch may delete a placeholder and insert the carried row with the
    // same id in that scheme. The final workspace still owns that id, so the
    // delete is not a tombstone intent for the CRDT document. Keep the marker
    // only for ids that remain absent after the batch; cross-scheme moves still
    // retain the source delete because their insert is in another scheme.
    let mut inserted_items: HashMap<SchemeId, HashSet<String>> = HashMap::new();
    collect_inserted_item_ids(command, &mut inserted_items);
    for (scheme, inserted) in inserted_items {
        if let Some(deleted) = deleted_items.get_mut(&scheme) {
            deleted.retain(|item| !inserted.contains(item));
        }
    }
    deleted_items.retain(|_, items| !items.is_empty());
    WorkspaceCrdtChangeSet {
        workspace: documents.workspace,
        schemes: documents.schemes.into_iter().collect(),
        deleted_items,
    }
}

fn collect_deleted_item_ids(
    command: &Command,
    deleted_items: &mut HashMap<SchemeId, HashSet<String>>,
) {
    match command {
        Command::DeleteItem { scheme, item } => {
            deleted_items
                .entry(*scheme)
                .or_default()
                .insert(item.to_string());
        }
        Command::Batch(commands) => {
            for command in commands {
                collect_deleted_item_ids(command, deleted_items);
            }
        }
        _ => {}
    }
}

fn collect_inserted_item_ids(
    command: &Command,
    inserted_items: &mut HashMap<SchemeId, HashSet<String>>,
) {
    match command {
        Command::InsertItem { scheme, item, .. } => {
            inserted_items
                .entry(*scheme)
                .or_default()
                .insert(item.id.to_string());
        }
        Command::Batch(commands) => {
            for command in commands {
                collect_inserted_item_ids(command, inserted_items);
            }
        }
        _ => {}
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
