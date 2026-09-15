use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, Workspace, WorkspaceId};
use serde::{Deserialize, Serialize};
use yrs::updates::{decoder::Decode, encoder::Encode};
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::{
    validate_crdt_update_sequence, CrdtDocumentUpdate, PushUpdatesRequest, SyncDocumentRef,
    WorkspaceCrdtDocuments, SYNC_STATE_RECOVERY_VERSION,
};

/// The persisted state of `document` with the queued `pending` edits to it
/// applied, or `None` when it already held every one of them.
///
/// The save task writes the pending queue before the CRDT state, so a crash
/// between the two leaves queued edits the saved document never saw. A session
/// restored from that state authors its next edits concurrently with them, and
/// once the queue is pushed a stale queued edit can win and revert what the user
/// did since. Applying an update the document already holds changes nothing, so
/// this only fills that gap — including in data directories an older build left
/// this way. A document with no saved state is left to the sync that seeds it.
pub fn fold_pending_edits_into_state<'a>(
    document: DocumentId,
    state: &[u8],
    pending: impl IntoIterator<Item = &'a PendingCrdtEdit>,
) -> Option<Vec<u8>> {
    let mut edits: Vec<&PendingCrdtEdit> = pending
        .into_iter()
        .filter(|edit| edit.document == document)
        .collect();
    if edits.is_empty() || state.is_empty() {
        return None;
    }
    edits.sort_by_key(|edit| edit.local_sequence);
    let doc = Doc::new();
    let mut txn = doc.transact_mut();
    txn.apply_update(Update::decode_v1(state).ok()?).ok()?;
    let before = txn.encode_state_as_update_v1(&StateVector::default());
    for edit in edits {
        // One unreadable queued edit must not cost the others.
        if let Ok(update) = Update::decode_v1(&edit.update_v1) {
            let _ = txn.apply_update(update);
        }
    }
    let after = txn.encode_state_as_update_v1(&StateVector::default());
    (after != before).then_some(after)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingCrdtEdit {
    pub operation_id: OperationId,
    pub workspace_id: WorkspaceId,
    pub replica_id: ReplicaId,
    pub local_sequence: u64,
    pub created_at: DateTime<Utc>,
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    #[serde(with = "crate::base64_bytes")]
    pub update_v1: Vec<u8>,
    /// Item ids this edit touched (see [`CrdtDocumentUpdate::touched_items`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_items: Vec<String>,
}

impl PendingCrdtEdit {
    pub fn as_update(&self) -> CrdtDocumentUpdate {
        CrdtDocumentUpdate {
            document: self.document,
            kind: self.kind,
            update_v1: self.update_v1.clone(),
            touched_items: self.touched_items.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentSyncCursor {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    #[serde(default)]
    pub last_pulled_sequence: u64,
    #[serde(default)]
    pub last_pushed_sequence: u64,
    /// The document epoch the last pulled state carried (0 until a squash ever
    /// happens). A pulled epoch differing from this triggers adoption-by-replace
    /// instead of a CRDT merge, and pushes carry it so the server can reject
    /// stale-epoch updates.
    #[serde(default)]
    pub epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MediaSyncCursor {
    pub image_name: String,
    pub document: DocumentId,
    pub byte_length: u64,
    #[serde(default)]
    pub sha256: String,
    pub uploaded_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalSyncState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replica_id: Option<ReplicaId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// SHA-256 of the access token that last established `workspace_id` with
    /// `server_url`. The bearer itself is never written here. Mobile uses this
    /// to reuse a still-valid session's canonical workspace id on cold launch;
    /// a rotated token or server change falls back to account status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_token_fingerprint: Option<String>,
    /// Stable account identifier supplied by the mobile shell. Access tokens
    /// rotate frequently; this lets a cold launch reuse the canonical workspace
    /// id without an account-status round trip, while an actual account switch
    /// still takes the authoritative lookup path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_user_id: Option<String>,
    #[serde(default)]
    pub document_cursors: HashMap<DocumentId, DocumentSyncCursor>,
    /// State-vector proofs from the last durable sync checkpoint. Mobile keeps
    /// these for deferred CRDT documents so a startup integrity check can ask
    /// the server about cold histories without decoding every one first.
    /// Missing entries are simply outside the proof scope and are covered by
    /// the normal per-document pull cursors.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub integrity_state_vectors: HashMap<DocumentId, String>,
    #[serde(default)]
    pub media_cursors: HashMap<String, MediaSyncCursor>,
    /// Last time the best-effort missing-media sweep was attempted. This is
    /// durable so restarting the app cannot turn the sweep into a blocking
    /// startup job every time. A remote document change or local push still
    /// triggers an immediate sweep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_media_reconciliation_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub pending: VecDeque<PendingCrdtEdit>,
    /// Last applied recovery generation (see [`SYNC_STATE_RECOVERY_VERSION`]).
    /// Absent in older files, so it defaults to 0 and triggers the heal.
    #[serde(default)]
    pub recovery_version: u32,
    /// Set when cursors were reset for an account/server change, cleared once
    /// this device has re-seeded full snapshots against the new server.
    ///
    /// Scheme and daily-queue content documents are keyed by *derived* ids, so
    /// the same document id exists on every account. After a switch this
    /// device's local document holds the old account's history while the new
    /// server holds an unrelated base under the same id — and an incremental
    /// `encode_diff_v1` against the local state vector assumes a receiver that
    /// already has that history. Applying such a delta to the new base can
    /// delete structs it never had (observed: the scheme's `schema` key), which
    /// the server rejects as `crdt_schema_invalid` — permanently, because the
    /// device just re-queues the same delta. While this is set every document is
    /// re-seeded as a full snapshot instead, which merges into any base.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reseed_all_documents: bool,
    /// Set before a sync starts and cleared once the merged workspace and pull
    /// cursors are durably paired. A process termination while this is set
    /// arms the next launch's one-shot integrity proof; clean launches stay
    /// cursor-only. Mobile submits cached state vectors for cold documents, so
    /// this recovery proof does not require decoding the whole workspace.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sync_in_progress: bool,
    /// Deferred scheme documents whose complete remote state changed during a
    /// lazy bootstrap. Mobile keeps their existing plain files cheap to load,
    /// but must hydrate the authoritative CRDT bytes when one of these schemes
    /// enters the visible daily range. Older state files simply have no set.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub deferred_materialization_pending: HashSet<DocumentId>,
}

impl LocalSyncState {
    pub fn is_configured(&self) -> bool {
        self.workspace_id.is_some()
            && self.replica_id.is_some()
            && self
                .server_url
                .as_deref()
                .is_some_and(|url| !url.is_empty())
    }

    pub fn mark_deferred_materialization(&mut self, document: DocumentId) {
        self.deferred_materialization_pending.insert(document);
    }

    pub fn clear_deferred_materialization(&mut self, document: DocumentId) -> bool {
        self.deferred_materialization_pending.remove(&document)
    }

    pub fn replace_pending(&mut self, pending: impl IntoIterator<Item = PendingCrdtEdit>) {
        self.pending = pending.into_iter().collect();
    }

    /// Clear pull/push and media cursors and drop stale workspace-index pending so
    /// the next sync re-pulls every document from sequence zero and re-seeds full
    /// snapshots (idempotent in Yjs). Workspace-index pending is dropped because it
    /// can encode deltas against a partial/corrupt or different-account workspace
    /// index; scheme content pending is kept — the bootstrap either re-pushes it as
    /// a valid self-contained sequence or replaces it with a full snapshot. Shared
    /// by the one-time recovery heal and the account-switch reset.
    fn clear_cursors_for_full_repull(&mut self) {
        self.document_cursors.clear();
        self.media_cursors.clear();
        self.deferred_materialization_pending.clear();
        self.last_media_reconciliation_at = None;
        self.pending
            .retain(|edit| edit.kind != SyncDocumentKind::PersonalWorkspace);
    }

    /// As [`clear_cursors_for_full_repull`], plus arming the full-snapshot
    /// re-seed an account/server change requires (see `reseed_all_documents`).
    fn clear_cursors_for_account_change(&mut self) {
        self.clear_cursors_for_full_repull();
        self.account_token_fingerprint = None;
        // Document ids for schemes/daily pages are derived from their logical
        // ids and can recur in another account. A cached Yjs state vector is
        // only meaningful for the history it was computed from, so never send
        // the previous account's vectors as delta hints after a switch.
        self.integrity_state_vectors.clear();
        self.reseed_all_documents = true;
    }

    /// Whether this device still owes the current server a full snapshot of
    /// every document.
    pub fn needs_full_reseed(&self) -> bool {
        self.reseed_all_documents
    }

    /// Clear the re-seed obligation once the snapshots have been queued.
    pub fn clear_full_reseed(&mut self) {
        self.reseed_all_documents = false;
    }

    /// Apply any pending one-time recovery for the current
    /// [`SYNC_STATE_RECOVERY_VERSION`]. Clears pull cursors so the next sync
    /// re-pulls every document from sequence zero and re-merges (idempotent in
    /// Yjs), repairing an on-disk workspace that diverged from advanced cursors.
    /// Workspace-index pending edits are dropped during recovery because older
    /// clients could queue deltas from a partial/corrupt workspace index. Scheme
    /// content edits are left intact. Returns `true` if a heal was applied.
    pub fn heal_for_recovery_version(&mut self) -> bool {
        if self.recovery_version >= SYNC_STATE_RECOVERY_VERSION {
            return false;
        }
        self.clear_cursors_for_full_repull();
        self.recovery_version = SYNC_STATE_RECOVERY_VERSION;
        true
    }

    /// Apply the current recovery generation without downloading documents the
    /// local CRDT store already owns. This is the mobile recovery path: the old
    /// materialization bug could have advanced a cursor for a document that was
    /// absent locally, so reset only those bindings (plus the workspace index).
    /// Existing CRDT bytes are already the exact history we want to retain, and
    /// ordinary cursor pulls still fetch any server sequence that has advanced.
    ///
    /// Workspace-index pending edits are dropped for the same reason as the full
    /// recovery method; the plain workspace/CRDT pair will re-bootstrap them if
    /// needed. Scheme pending edits remain pushable.
    pub fn heal_for_recovery_version_targeted(
        &mut self,
        workspace: &Workspace,
        known_document_ids: &HashSet<DocumentId>,
    ) -> bool {
        if self.recovery_version >= SYNC_STATE_RECOVERY_VERSION {
            return false;
        }
        self.pending
            .retain(|edit| edit.kind != SyncDocumentKind::PersonalWorkspace);
        self.reset_pull_cursor(workspace.sync.id);
        for meta in workspace.scheme_sync.values() {
            if meta.kind == SyncDocumentKind::Scheme && !known_document_ids.contains(&meta.id) {
                self.reset_pull_cursor(meta.id);
            }
        }
        self.recovery_version = SYNC_STATE_RECOVERY_VERSION;
        true
    }

    /// Reset cursors when signing in under a different account or server than these
    /// cursors were built against. The persisted `sync-state.json` is a single,
    /// account-agnostic file, so without this an account switch (sign out of A, sign
    /// into B) reuses account A's pull/push and media cursors. A carried-over cursor
    /// is unsafe two ways:
    ///
    /// 1. **Silent data loss on pull** — the pull request is keyed by document with
    ///    A's `last_pulled_sequence`; for a document B holds at a lower sequence the
    ///    server returns nothing, so B's content is never pulled.
    /// 2. **`crdt_schema_invalid` on push** — a non-zero cursor makes the bootstrap
    ///    treat a document B has no base for as already-present and push a bare delta
    ///    instead of a full snapshot. Reconstructed from empty on the server, the
    ///    delta has no `schema` root and the backend rejects the whole batch.
    ///
    /// Resetting forces the next sync to re-pull every document from sequence zero
    /// and re-seed full snapshots, which Yjs merges idempotently (the workspace doc
    /// itself is re-keyed and re-queued separately by the caller). No-op (returns
    /// `false`) on first configuration (no prior identity recorded) or when both the
    /// account workspace id and server url are unchanged.
    pub fn reset_for_account_change(
        &mut self,
        new_workspace_id: WorkspaceId,
        new_server_url: &str,
    ) -> bool {
        let workspace_changed = self
            .workspace_id
            .is_some_and(|existing| existing != new_workspace_id);
        let server_changed = self
            .server_url
            .as_deref()
            .is_some_and(|existing| existing != new_server_url);
        if !(workspace_changed || server_changed) {
            return false;
        }
        self.clear_cursors_for_account_change();
        true
    }

    pub fn push_pending(&mut self, edit: PendingCrdtEdit) {
        self.pending.push_back(edit);
    }

    pub fn pending_for_document(&self, document: DocumentId, limit: usize) -> Vec<PendingCrdtEdit> {
        self.pending
            .iter()
            .filter(|edit| edit.document == document)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn pending_document_sequence_is_valid(
        &self,
        document: DocumentId,
        kind: SyncDocumentKind,
    ) -> bool {
        let updates = self
            .pending
            .iter()
            .filter(|edit| edit.document == document)
            .map(|edit| edit.update_v1.as_slice())
            .collect::<Vec<_>>();
        !updates.is_empty() && validate_crdt_update_sequence(kind, updates).is_ok()
    }

    pub fn should_upsert_document(&self, doc: SyncDocumentRef) -> bool {
        !self.document_cursors.contains_key(&doc.document)
    }

    pub fn next_push_request(
        &self,
        document: DocumentId,
        limit: usize,
    ) -> Option<PushUpdatesRequest> {
        let replica_id = self.replica_id?;
        let updates = self
            .pending_for_document(document, limit)
            .into_iter()
            .map(|edit| edit.as_update())
            .collect::<Vec<_>>();
        if updates.is_empty() {
            return None;
        }
        Some(PushUpdatesRequest {
            replica_id,
            updates,
            notification_schedule_changed: false,
            notification_schedule: None,
        })
    }

    /// Clear the **first contiguous prefix** of pending edits for `document` whose
    /// sequences are <= `through_local_sequence`, stopping after the first edit that
    /// has `local_sequence == through_local_sequence`. Edits that appear later in
    /// the deque with the same sequence numbers (from a legacy restart that reset
    /// `next_sequence` to 1) are left intact because they were never sent.
    pub fn mark_pushed(&mut self, document: DocumentId, through_local_sequence: u64) -> usize {
        let before = self.pending.len();
        let mut kind = None;
        let mut done = false;
        self.pending.retain(|edit| {
            if done {
                return true;
            }
            if edit.document == document && edit.local_sequence <= through_local_sequence {
                kind = Some(edit.kind);
                if edit.local_sequence == through_local_sequence {
                    done = true;
                }
                false
            } else {
                true
            }
        });
        if let Some(kind) = kind {
            let cursor = self
                .document_cursors
                .entry(document)
                .or_insert(DocumentSyncCursor {
                    document,
                    kind,
                    last_pulled_sequence: 0,
                    last_pushed_sequence: 0,
                    epoch: 0,
                });
            cursor.last_pushed_sequence = cursor.last_pushed_sequence.max(through_local_sequence);
        }
        before - self.pending.len()
    }

    /// Clear exactly the pending edits identified by `(operation_id, local_sequence)` pairs
    /// for `document`, advancing the pushed cursor to `max(existing, max sent seq)`.
    /// Used by the engine to clear precisely the edits a server-acknowledged batch contained,
    /// even when duplicate sequences are present.
    pub fn mark_pushed_edits(&mut self, document: DocumentId, edits: &[(OperationId, u64)]) {
        if edits.is_empty() {
            return;
        }
        let sent: HashSet<(OperationId, u64)> = edits.iter().copied().collect();
        let max_seq = edits.iter().map(|(_, seq)| *seq).max().unwrap_or(0);
        let mut kind = None;
        self.pending.retain(|edit| {
            if edit.document == document && sent.contains(&(edit.operation_id, edit.local_sequence))
            {
                kind = Some(edit.kind);
                false
            } else {
                true
            }
        });
        if let Some(kind) = kind {
            let cursor = self
                .document_cursors
                .entry(document)
                .or_insert(DocumentSyncCursor {
                    document,
                    kind,
                    last_pulled_sequence: 0,
                    last_pushed_sequence: 0,
                    epoch: 0,
                });
            cursor.last_pushed_sequence = cursor.last_pushed_sequence.max(max_seq);
        }
    }

    pub fn mark_pulled(
        &mut self,
        document: DocumentId,
        kind: SyncDocumentKind,
        latest_sequence: u64,
        epoch: u64,
    ) {
        let cursor = self
            .document_cursors
            .entry(document)
            .or_insert(DocumentSyncCursor {
                document,
                kind,
                last_pulled_sequence: 0,
                last_pushed_sequence: 0,
                epoch: 0,
            });
        cursor.kind = kind;
        cursor.last_pulled_sequence = cursor.last_pulled_sequence.max(latest_sequence);
        cursor.epoch = epoch;
    }

    /// Advance a post-push pull cursor to the exact server head returned by the
    /// push response, without changing the epoch learned from the last pull.
    /// The caller must still run the scoped integrity proof before making this
    /// state durable: another device may have changed the document immediately
    /// after the push response was committed.
    pub fn advance_pushed_server_sequence(
        &mut self,
        document: DocumentId,
        kind: SyncDocumentKind,
        server_sequence: u64,
    ) {
        let cursor = self
            .document_cursors
            .entry(document)
            .or_insert(DocumentSyncCursor {
                document,
                kind,
                last_pulled_sequence: 0,
                last_pushed_sequence: 0,
                epoch: 0,
            });
        cursor.kind = kind;
        cursor.last_pulled_sequence = cursor.last_pulled_sequence.max(server_sequence);
    }

    /// The epoch this replica last recorded for `document` (0 when unknown).
    pub fn document_epoch(&self, document: DocumentId) -> u64 {
        self.document_cursors
            .get(&document)
            .map(|cursor| cursor.epoch)
            .unwrap_or(0)
    }

    /// The union of item ids touched by the pending edits for `document`, for
    /// the adoption rescue.
    pub fn pending_touched_items(&self, document: DocumentId) -> HashSet<String> {
        self.pending
            .iter()
            .filter(|edit| edit.document == document)
            .flat_map(|edit| edit.touched_items.iter().cloned())
            .collect()
    }

    pub fn has_pending_for_document(&self, document: DocumentId) -> bool {
        self.pending.iter().any(|edit| edit.document == document)
    }

    pub fn media_upload_is_current(
        &self,
        image_name: &str,
        document: DocumentId,
        byte_length: u64,
        sha256: &str,
    ) -> bool {
        self.media_cursors.get(image_name).is_some_and(|cursor| {
            cursor.document == document
                && cursor.byte_length == byte_length
                && cursor.sha256 == sha256
        })
    }

    pub fn should_upload_media_asset(
        &self,
        image_name: &str,
        document: DocumentId,
        byte_length: u64,
        sha256: &str,
        remote_latest: &HashMap<DocumentId, u64>,
    ) -> bool {
        remote_latest.get(&document).copied().unwrap_or(0) == 0
            || !self.media_upload_is_current(image_name, document, byte_length, sha256)
    }

    /// Reset the pull cursor for `document` to 0, forcing a full re-pull next
    /// cycle. Used after the workspace index is updated to include a scheme whose
    /// content document was previously skipped (cursor advanced past content we
    /// could not apply). Resetting forces re-convergence without infinite-looping
    /// within the current call: we only reset; the next poll re-pulls.
    pub fn reset_pull_cursor(&mut self, document: DocumentId) {
        if let Some(cursor) = self.document_cursors.get_mut(&document) {
            cursor.last_pulled_sequence = 0;
        }
        // If there is no cursor yet the next pull will already fetch from seq 0.
    }

    /// Forget only the personal-workspace cursor after its on-disk index was
    /// unreadable and replaced. The replacement has no trustworthy relation to
    /// the prior index, but scheme cursors and their pending local edits remain
    /// valid. Pulling the workspace index from zero lets the engine discover
    /// precisely which scheme cursors also need re-fetching.
    pub fn reset_workspace_pull_cursor(&mut self) -> bool {
        let before = self.document_cursors.len();
        self.document_cursors
            .retain(|_, cursor| cursor.kind != SyncDocumentKind::PersonalWorkspace);
        self.document_cursors.len() != before
    }

    /// Reconcile cached pull cursors against the server's authoritative heads.
    ///
    /// A cursor greater than the server's sequence is impossible unless local
    /// state was damaged or the server was reset; reset only that document so
    /// the next request receives its full merged state. A cursor for a document
    /// the server no longer has must be removed entirely: leaving it at zero
    /// would make every future head comparison repeat without making progress.
    /// The caller's bootstrap path will re-seed a locally-held document that is
    /// absent from the server. Returns true when another pull is needed now.
    pub fn reconcile_server_heads(&mut self, heads: &HashMap<DocumentId, u64>) -> bool {
        let mut needs_repull = false;
        self.document_cursors
            .retain(|document, cursor| match heads.get(document) {
                Some(server_sequence) if cursor.last_pulled_sequence > *server_sequence => {
                    cursor.last_pulled_sequence = 0;
                    needs_repull = true;
                    true
                }
                Some(_) => true,
                None => {
                    needs_repull = true;
                    false
                }
            });
        needs_repull
    }

    pub fn mark_media_uploaded(
        &mut self,
        image_name: String,
        document: DocumentId,
        byte_length: u64,
        sha256: String,
    ) {
        self.media_cursors.insert(
            image_name.clone(),
            MediaSyncCursor {
                image_name,
                document,
                byte_length,
                sha256,
                uploaded_at: Utc::now(),
            },
        );
    }
}

pub fn queue_workspace_bootstrap_updates(
    sync_state: &mut LocalSyncState,
    crdt: &mut WorkspaceCrdtDocuments,
    workspace: &Workspace,
    replica_id: ReplicaId,
    remote_latest: &HashMap<DocumentId, u64>,
) -> Vec<DocumentId> {
    let reseed_all = sync_state.needs_full_reseed();
    // A normal pull only needs to bootstrap documents for which the server has
    // no base. Computing this set from the authoritative server heads lets the
    // CRDT layer skip full-state encoding for every already-synced document.
    // Account/server reseeds are intentionally exhaustive and remain rare.
    let bootstrap_documents: HashSet<DocumentId> = crdt
        .known_document_ids()
        .into_iter()
        .filter(|document| reseed_all || remote_latest.get(document).copied().unwrap_or(0) == 0)
        .collect();
    // Before snapshotting, repair any document whose full state would fail the
    // server's schema validation — a scheme added to the workspace outside the
    // command path (e.g. desktop's direct Daily Queue creation) leaves an empty
    // Yjs doc whose snapshot the server rejects as `crdt_schema_invalid`, wedging
    // the whole push batch. Only documents the server has no base for are
    // eligible, so a heal never competes with un-pulled server content.
    // Only documents the server has no base for are eligible, so a heal never
    // competes with un-pulled server content. (An item left as a schema-less partial
    // by a multi-origin merge no longer needs healing here: validation now tolerates
    // partial items and materialization skips them identically on every replica, so
    // the snapshot pushes fine and all replicas converge — see
    // validate_scheme_document. Healing here is now only for an empty, schema-less
    // document, e.g. desktop's direct Daily Queue creation before its first pull.)
    let healed = crdt.heal_schema_invalid_documents_for_documents(workspace, &bootstrap_documents);
    let healed_set: HashSet<DocumentId> = healed.iter().copied().collect();
    let mut next_sequence = sync_state
        .pending
        .iter()
        .map(|edit| edit.local_sequence)
        .max()
        .unwrap_or(0)
        + 1;
    let mut bootstrapped: HashSet<DocumentId> = HashSet::new();
    // Re-seed full snapshots from the live, persistent documents so the base the
    // server rebuilds shares clientID + clocks with this device's incremental diffs
    // (a throwaway snapshot would carry a fresh identity that competes with them).
    // After an account/server change this device shares no history with the new
    // server's documents, so an incremental delta against the local state vector
    // is not applicable there — every document must go out as a full snapshot,
    // even one the server already has a base for (a snapshot merges into any
    // base; a foreign-history delta corrupts it). See `reseed_all_documents`.
    for update in crdt
        .full_snapshot_updates_for_documents(&bootstrap_documents)
        .updates
    {
        // Only documents the server lacks a base for are seeded here; a document the
        // server already holds converges through the normal pull/push CRDT merge.
        if !reseed_all && remote_latest.get(&update.document).copied().unwrap_or(0) != 0 {
            continue;
        }
        // A just-healed document's queued edits predate the heal (they are the
        // schema-less updates the server rejected) — replace them with the healed
        // snapshot instead of trusting them.
        if !healed_set.contains(&update.document)
            && sync_state.pending_document_sequence_is_valid(update.document, update.kind)
        {
            bootstrapped.insert(update.document);
            continue;
        }
        // If local deltas were queued before the first successful upload, they
        // cannot be applied on the server without a base document. Trust the
        // server's zero sequence over any stale local cursor, then push the
        // current full snapshot first.
        sync_state
            .pending
            .retain(|pending| pending.document != update.document);
        bootstrapped.insert(update.document);
        sync_state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace.id,
            replica_id,
            local_sequence: next_sequence,
            created_at: Utc::now(),
            document: update.document,
            kind: update.kind,
            update_v1: update.update_v1,
            touched_items: update.touched_items,
        });
        next_sequence += 1;
    }
    // The obligation means "queue a full snapshot once after an account/server
    // change", not "force a full snapshot on every later poll". The queued
    // snapshots are durable pending edits and remain retryable if the network
    // push fails or accepts only part of the batch.
    if reseed_all {
        sync_state.clear_full_reseed();
    }

    // Drop queued deltas that the server can never accept: a document it has no
    // base snapshot for (remote sequence 0) that we also did not just re-seed with
    // a full snapshot above. These orphans appear when a scheme is deleted or its
    // sync-document id is reassigned while edits are still queued. A lone delta
    // reconstructs a document with no `schema` field, which the backend rejects as
    // `crdt_schema_invalid`, wedging the push loop behind the bad edit.
    sync_state.pending.retain(|edit| {
        bootstrapped.contains(&edit.document)
            || remote_latest.get(&edit.document).copied().unwrap_or(0) != 0
    });

    healed
}

/// Force-queue a full snapshot for every scheme content document, so an account switch
/// re-seeds this device's content to the new account even for schemes the new server
/// already holds (from another origin or empty). [`queue_workspace_bootstrap_updates`]
/// alone only re-seeds schemes the server LACKS (remote seq 0); without this, content
/// already pushed to the previous account never reaches the new one — the cross-account
/// content gap (a device shows lines the new account never receives). Full snapshots
/// union idempotently on the server, and with deterministic item creation items dedupe
/// rather than duplicate. Call after pulling the destination workspace;
/// `excluded_documents` contains documents that pull could not safely merge and
/// therefore must not be re-seeded from an incompatible account history.
pub fn queue_account_switch_reseed(
    sync_state: &mut LocalSyncState,
    crdt: &WorkspaceCrdtDocuments,
    workspace: &Workspace,
    replica_id: ReplicaId,
    excluded_documents: &HashSet<DocumentId>,
) {
    // Pending scheme edits survive the cursor reset so local content can follow the
    // user to the destination account. After its workspace index has been pulled,
    // however, a scheme absent from that merged index is an orphan (for example a
    // source-only scheme that the destination history already tombstoned). Never
    // push such a document: its destination base may legitimately be schema-less,
    // and it is no longer addressable from the workspace in any case.
    let indexed_scheme_documents: HashSet<DocumentId> = workspace
        .scheme_sync
        .values()
        .map(|metadata| metadata.id)
        .collect();
    sync_state.pending.retain(|edit| {
        edit.kind != SyncDocumentKind::Scheme || indexed_scheme_documents.contains(&edit.document)
    });

    let mut next_sequence = sync_state
        .pending
        .iter()
        .map(|edit| edit.local_sequence)
        .max()
        .unwrap_or(0)
        + 1;
    for update in crdt.full_snapshot_updates().updates {
        if update.kind != SyncDocumentKind::Scheme
            || !indexed_scheme_documents.contains(&update.document)
            || excluded_documents.contains(&update.document)
        {
            continue;
        }
        sync_state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace.id,
            replica_id,
            local_sequence: next_sequence,
            created_at: Utc::now(),
            document: update.document,
            kind: update.kind,
            update_v1: update.update_v1,
            touched_items: update.touched_items,
        });
        next_sequence += 1;
    }
}

/// Beyond this many queued edits for one document, merging them costs less than
/// carrying them separately. Generous on purpose: merging decodes and re-encodes
/// the whole backlog, so it should be rare relative to editing.
pub const MAX_PENDING_PER_DOCUMENT: usize = 32;

/// Merge a document's queued deltas into one equivalent update once there are
/// too many, and report how many documents were compacted.
///
/// The queue only drains on a successful push. A device that cannot push —
/// signed out, offline for a long stretch, or running a build with accounts
/// compiled out — otherwise accumulates one entry per edit forever, and the
/// whole file is re-read and re-written on every later edit, so editing gets
/// steadily slower the longer it goes unsynced.
///
/// Lossless by construction: `Update::merge_updates` produces a single update
/// with the same effect as applying the originals in order, and — unlike
/// substituting a full snapshot — it needs no live document and no base beyond
/// the one the first delta already needed. That matters because most of a real
/// backlog belongs to daily-queue schemes that are loaded lazily and so have no
/// document in memory to snapshot from; dropping those entries instead would
/// silently discard edits.
///
/// A document whose updates fail to decode or merge is left exactly as it was:
/// a queue that cannot be compacted is a performance problem, but discarding it
/// would be a data-loss one.
pub fn compact_pending_documents(
    sync_state: &mut LocalSyncState,
    max_pending_per_document: usize,
) -> usize {
    let mut counts: HashMap<DocumentId, usize> = HashMap::new();
    for edit in &sync_state.pending {
        *counts.entry(edit.document).or_default() += 1;
    }
    let overfull: Vec<DocumentId> = counts
        .into_iter()
        .filter(|(_, count)| *count > max_pending_per_document)
        .map(|(document, _)| document)
        .collect();
    if overfull.is_empty() {
        return 0;
    }

    let mut compacted = 0;
    for document in overfull {
        let Some(merged) = merge_document_pending(sync_state, document) else {
            continue;
        };
        sync_state.pending.retain(|edit| edit.document != document);
        sync_state.push_pending(merged);
        compacted += 1;
    }
    if compacted > 0 {
        // `push_pending` appends, but a merged entry inherits the sequence of the
        // last edit it replaced, so restore submission order across documents.
        sync_state
            .pending
            .make_contiguous()
            .sort_by_key(|edit| edit.local_sequence);
    }
    compacted
}

/// One update equivalent to every queued edit for `document`, or `None` if they
/// cannot be decoded and merged.
fn merge_document_pending(
    sync_state: &LocalSyncState,
    document: DocumentId,
) -> Option<PendingCrdtEdit> {
    let queued: Vec<&PendingCrdtEdit> = sync_state
        .pending
        .iter()
        .filter(|edit| edit.document == document)
        .collect();
    let last = queued.last().copied()?;

    let mut updates = Vec::with_capacity(queued.len());
    for edit in &queued {
        updates.push(Update::decode_v1(&edit.update_v1).ok()?);
    }
    let merged = Update::merge_updates(updates);

    // The union of everything the merged edit now carries, for the epoch
    // adoption rescue — order-preserving so it stays deterministic.
    let mut touched_items = Vec::new();
    let mut seen = HashSet::new();
    for edit in &queued {
        for item in &edit.touched_items {
            if seen.insert(item.clone()) {
                touched_items.push(item.clone());
            }
        }
    }

    Some(PendingCrdtEdit {
        operation_id: OperationId::new(),
        workspace_id: last.workspace_id,
        replica_id: last.replica_id,
        // Keep the newest sequence so the merged edit sits where the backlog
        // ended, relative to other documents' queued edits.
        local_sequence: last.local_sequence,
        created_at: last.created_at,
        document,
        kind: last.kind,
        update_v1: merged.encode_v1(),
        touched_items,
    })
}

#[cfg(test)]
mod account_change_tests {
    use std::collections::{HashMap, HashSet};

    use super::{
        queue_account_switch_reseed, queue_workspace_bootstrap_updates, DocumentSyncCursor,
        LocalSyncState, MediaSyncCursor, PendingCrdtEdit,
    };
    use crate::WorkspaceCrdtDocuments;
    use chrono::Utc;
    use knotq_model::{
        DocumentId, OperationId, ReplicaId, Scheme, SyncDocumentKind, Workspace, WorkspaceId,
    };

    const SERVER_A: &str = "https://a.api.knotq.com";
    const SERVER_B: &str = "https://b.api.knotq.com";

    fn pending(
        workspace: WorkspaceId,
        document: DocumentId,
        kind: SyncDocumentKind,
    ) -> PendingCrdtEdit {
        PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace,
            replica_id: ReplicaId::new(),
            local_sequence: 1,
            created_at: Utc::now(),
            document,
            kind,
            update_v1: vec![1, 2, 3],
            touched_items: Vec::new(),
        }
    }

    /// A fully-configured state for account `workspace`/`server` carrying a scheme
    /// cursor, a media cursor, plus one scheme and one workspace pending edit.
    fn configured_state(workspace: WorkspaceId, server: &str) -> LocalSyncState {
        let scheme_doc = DocumentId::new();
        let workspace_doc = DocumentId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace),
            replica_id: Some(ReplicaId::new()),
            server_url: Some(server.to_string()),
            ..LocalSyncState::default()
        };
        state.document_cursors.insert(
            scheme_doc,
            DocumentSyncCursor {
                document: scheme_doc,
                kind: SyncDocumentKind::Scheme,
                last_pulled_sequence: 4,
                last_pushed_sequence: 4,
                epoch: 0,
            },
        );
        state.media_cursors.insert(
            "image.png".to_string(),
            MediaSyncCursor {
                image_name: "image.png".to_string(),
                document: scheme_doc,
                byte_length: 3,
                sha256: "deadbeef".to_string(),
                uploaded_at: Utc::now(),
            },
        );
        state.push_pending(pending(workspace, scheme_doc, SyncDocumentKind::Scheme));
        state.push_pending(pending(
            workspace,
            workspace_doc,
            SyncDocumentKind::PersonalWorkspace,
        ));
        state
    }

    #[test]
    fn resets_cursors_when_workspace_id_changes() {
        let account_a = WorkspaceId::new();
        let account_b = WorkspaceId::new();
        let mut state = configured_state(account_a, SERVER_A);
        state.account_token_fingerprint = Some("old-token-fingerprint".to_string());

        assert!(state.reset_for_account_change(account_b, SERVER_A));

        assert!(
            state.document_cursors.is_empty(),
            "pull/push cursors cleared"
        );
        assert!(state.media_cursors.is_empty(), "media cursors cleared");
        assert!(
            state.account_token_fingerprint.is_none(),
            "the new account must not inherit the old token binding"
        );
        // Scheme content pending is kept; workspace-index pending is dropped.
        assert_eq!(state.pending.len(), 1);
        assert!(state
            .pending
            .iter()
            .all(|edit| edit.kind == SyncDocumentKind::Scheme));
    }

    #[test]
    fn media_reconciliation_timestamp_is_durable_and_cleared_by_full_recovery() {
        let account_a = WorkspaceId::new();
        let account_b = WorkspaceId::new();
        let mut state = configured_state(account_a, SERVER_A);
        let checked_at = Utc::now();
        state.last_media_reconciliation_at = Some(checked_at);

        let json = serde_json::to_string(&state).expect("serialize");
        let restored: LocalSyncState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.last_media_reconciliation_at, Some(checked_at));

        assert!(state.reset_for_account_change(account_b, SERVER_A));
        assert!(state.last_media_reconciliation_at.is_none());
    }

    /// An account/server change must arm the full-snapshot re-seed.
    ///
    /// Scheme and daily-queue documents carry *derived* ids, so the same
    /// document id exists on the new account with a completely different
    /// history. Pushing an incremental delta against it made the server reject
    /// the batch with `crdt_schema_invalid` — permanently, since the device just
    /// re-queued the same delta (found by the property fuzzer at depth,
    /// `account_hopping_fuzz_converges`). While the flag is set, every document
    /// goes out as a full snapshot, which merges into any base.
    #[test]
    fn an_account_change_arms_the_full_snapshot_reseed() {
        let account_a = WorkspaceId::new();
        let account_b = WorkspaceId::new();
        let mut state = configured_state(account_a, SERVER_A);
        assert!(
            !state.needs_full_reseed(),
            "a steady-state device owes no re-seed"
        );

        assert!(state.reset_for_account_change(account_b, SERVER_A));
        assert!(
            state.needs_full_reseed(),
            "switching account must force full snapshots, not incremental deltas"
        );

        state.clear_full_reseed();
        assert!(!state.needs_full_reseed());
    }

    #[test]
    fn targeted_recovery_resets_only_crdt_documents_missing_locally() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Visible", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        let missing_scheme = Scheme::new("Missing", 1);
        let missing_scheme_id = missing_scheme.id;
        workspace.schemes.insert(missing_scheme_id, missing_scheme);
        workspace.ensure_sync_metadata();
        let workspace_document = workspace.sync.id;
        let scheme_document = workspace.scheme_sync[&scheme_id].id;
        let missing_document = workspace.scheme_sync[&missing_scheme_id].id;
        let mut state = LocalSyncState {
            workspace_id: Some(workspace.id),
            server_url: Some(SERVER_A.to_string()),
            ..LocalSyncState::default()
        };
        for document in [workspace_document, scheme_document, missing_document] {
            state.document_cursors.insert(
                document,
                DocumentSyncCursor {
                    document,
                    kind: if document == workspace_document {
                        SyncDocumentKind::PersonalWorkspace
                    } else {
                        SyncDocumentKind::Scheme
                    },
                    last_pulled_sequence: 8,
                    last_pushed_sequence: 8,
                    epoch: 0,
                },
            );
        }
        state.push_pending(pending(
            workspace.id,
            workspace_document,
            SyncDocumentKind::PersonalWorkspace,
        ));
        state.push_pending(pending(
            workspace.id,
            scheme_document,
            SyncDocumentKind::Scheme,
        ));
        state.media_cursors.insert(
            "image.png".to_string(),
            MediaSyncCursor {
                image_name: "image.png".to_string(),
                document: scheme_document,
                byte_length: 3,
                sha256: "deadbeef".to_string(),
                uploaded_at: Utc::now(),
            },
        );

        assert!(state.heal_for_recovery_version_targeted(
            &workspace,
            &HashSet::from([workspace_document, scheme_document])
        ));
        assert_eq!(
            state.document_cursors[&workspace_document].last_pulled_sequence,
            0
        );
        assert_eq!(
            state.document_cursors[&scheme_document].last_pulled_sequence,
            8
        );
        assert_eq!(
            state.document_cursors[&missing_document].last_pulled_sequence,
            0
        );
        assert!(state
            .pending
            .iter()
            .all(|edit| edit.kind == SyncDocumentKind::Scheme));
        assert_eq!(state.media_cursors.len(), 1);
    }

    /// Changing only the backend (prod -> sandbox) is the same hazard: a
    /// different server holds different history under the same document ids.
    #[test]
    fn a_server_change_arms_the_full_snapshot_reseed() {
        let account = WorkspaceId::new();
        let mut state = configured_state(account, SERVER_A);
        assert!(state.reset_for_account_change(account, SERVER_B));
        assert!(state.needs_full_reseed());
    }

    /// A no-op reset must not arm it — re-seeding every document on every sync
    /// would push the whole workspace repeatedly.
    #[test]
    fn no_reseed_is_armed_without_an_actual_account_change() {
        let account = WorkspaceId::new();
        let mut state = configured_state(account, SERVER_A);
        assert!(!state.reset_for_account_change(account, SERVER_A));
        assert!(!state.needs_full_reseed());
    }

    /// The flag has to survive a restart: the device still owes the new server
    /// full snapshots even if it is closed before the first post-switch sync.
    #[test]
    fn the_reseed_obligation_round_trips_through_persistence() {
        let account_a = WorkspaceId::new();
        let account_b = WorkspaceId::new();
        let mut state = configured_state(account_a, SERVER_A);
        state.reset_for_account_change(account_b, SERVER_A);

        let json = serde_json::to_string(&state).expect("serialize");
        let restored: LocalSyncState = serde_json::from_str(&json).expect("deserialize");
        assert!(
            restored.needs_full_reseed(),
            "a device closed mid-switch must still re-seed on next launch"
        );

        // And a state that owes nothing stays quiet across a round trip, so the
        // flag never gets stuck on for existing installs.
        let mut settled = configured_state(account_b, SERVER_A);
        settled.clear_full_reseed();
        let json = serde_json::to_string(&settled).expect("serialize");
        assert!(
            !json.contains("reseed_all_documents"),
            "the default must not be written out"
        );
        let restored: LocalSyncState = serde_json::from_str(&json).expect("deserialize");
        assert!(!restored.needs_full_reseed());
    }

    #[test]
    fn resets_cursors_when_server_url_changes() {
        let account = WorkspaceId::new();
        let mut state = configured_state(account, SERVER_A);

        // Same workspace id but a different backend (prod -> sandbox).
        assert!(state.reset_for_account_change(account, SERVER_B));
        assert!(state.document_cursors.is_empty());
        assert!(state.media_cursors.is_empty());
    }

    #[test]
    fn no_reset_when_account_and_server_unchanged() {
        let account = WorkspaceId::new();
        let mut state = configured_state(account, SERVER_A);

        assert!(!state.reset_for_account_change(account, SERVER_A));
        assert_eq!(state.document_cursors.len(), 1);
        assert_eq!(state.media_cursors.len(), 1);
        assert_eq!(state.pending.len(), 2);
    }

    #[test]
    fn server_head_reconciliation_repairs_only_impossible_cursors() {
        let account = WorkspaceId::new();
        let mut state = configured_state(account, SERVER_A);
        let kept = DocumentId::new();
        state.document_cursors.insert(
            kept,
            DocumentSyncCursor {
                document: kept,
                kind: SyncDocumentKind::Scheme,
                last_pulled_sequence: 2,
                last_pushed_sequence: 2,
                epoch: 0,
            },
        );
        let stale = *state
            .document_cursors
            .keys()
            .find(|id| **id != kept)
            .unwrap();

        assert!(state.reconcile_server_heads(&HashMap::from([(kept, 2), (stale, 3)])));
        assert_eq!(state.document_cursors[&kept].last_pulled_sequence, 2);
        assert_eq!(state.document_cursors[&stale].last_pulled_sequence, 0);
        assert_eq!(
            state.pending.len(),
            2,
            "cursor repair must preserve local edits"
        );
    }

    #[test]
    fn server_head_reconciliation_drops_a_cursor_for_a_missing_remote_document() {
        let account = WorkspaceId::new();
        let mut state = configured_state(account, SERVER_A);
        let document = *state.document_cursors.keys().next().unwrap();

        assert!(state.reconcile_server_heads(&HashMap::new()));
        assert!(!state.document_cursors.contains_key(&document));
        assert_eq!(
            state.pending.len(),
            2,
            "pending edits are retained for bootstrap"
        );
    }

    #[test]
    fn workspace_parse_recovery_resets_only_the_workspace_cursor() {
        let account = WorkspaceId::new();
        let mut state = configured_state(account, SERVER_A);
        let workspace_document = DocumentId::new();
        state.document_cursors.insert(
            workspace_document,
            DocumentSyncCursor {
                document: workspace_document,
                kind: SyncDocumentKind::PersonalWorkspace,
                last_pulled_sequence: 7,
                last_pushed_sequence: 7,
                epoch: 0,
            },
        );
        let pending_before = state.pending.clone();

        assert!(state.reset_workspace_pull_cursor());
        assert!(!state.document_cursors.contains_key(&workspace_document));
        assert_eq!(state.document_cursors.len(), 1, "scheme cursor is retained");
        assert_eq!(state.pending, pending_before, "pending edits are retained");
    }

    #[test]
    fn no_reset_on_first_configuration() {
        // A fresh state has no recorded identity, so the first sign-in must not be
        // mistaken for an account switch (which would clear freshly-seeded cursors).
        let mut state = LocalSyncState::default();
        let scheme_doc = DocumentId::new();
        state.document_cursors.insert(
            scheme_doc,
            DocumentSyncCursor {
                document: scheme_doc,
                kind: SyncDocumentKind::Scheme,
                last_pulled_sequence: 0,
                last_pushed_sequence: 0,
                epoch: 0,
            },
        );
        assert!(!state.reset_for_account_change(WorkspaceId::new(), SERVER_A));
        assert_eq!(state.document_cursors.len(), 1);
    }

    #[test]
    fn reset_preserves_every_scheme_pending_and_drops_every_workspace_pending() {
        let account_a = WorkspaceId::new();
        let account_b = WorkspaceId::new();
        let mut state = configured_state(account_a, SERVER_A);
        // Add extra pending so we cover "many" rather than one of each.
        let scheme_doc = DocumentId::new();
        state.push_pending(pending(account_a, scheme_doc, SyncDocumentKind::Scheme));
        state.push_pending(pending(account_a, scheme_doc, SyncDocumentKind::Scheme));
        state.push_pending(pending(
            account_a,
            DocumentId::new(),
            SyncDocumentKind::PersonalWorkspace,
        ));

        assert!(state.reset_for_account_change(account_b, SERVER_A));

        assert!(state
            .pending
            .iter()
            .all(|edit| edit.kind == SyncDocumentKind::Scheme));
        assert_eq!(
            state.pending.len(),
            3,
            "the original scheme pending plus the two added ones survive"
        );
    }

    #[test]
    fn reset_is_safe_on_a_state_with_no_cursors() {
        let account_a = WorkspaceId::new();
        let account_b = WorkspaceId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(account_a),
            replica_id: Some(ReplicaId::new()),
            server_url: Some(SERVER_A.to_string()),
            ..LocalSyncState::default()
        };
        // Detects the change and is a no-op on the (already empty) cursor maps.
        assert!(state.reset_for_account_change(account_b, SERVER_A));
        assert!(state.document_cursors.is_empty());
        assert!(state.media_cursors.is_empty());
        assert!(state.pending.is_empty());
    }

    #[test]
    fn reset_triggers_when_both_account_and_server_change() {
        let account_a = WorkspaceId::new();
        let account_b = WorkspaceId::new();
        let mut state = configured_state(account_a, SERVER_A);
        assert!(state.reset_for_account_change(account_b, SERVER_B));
        assert!(state.document_cursors.is_empty());
        assert!(state.media_cursors.is_empty());
    }

    #[test]
    fn account_switch_reseed_drops_pending_orphan_scheme_documents() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Indexed", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let indexed = workspace.scheme_sync[&scheme_id].id;
        let orphan = DocumentId::new();
        let replica = ReplicaId::new();
        let crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        let mut state = LocalSyncState::default();
        state.push_pending(pending(workspace.id, indexed, SyncDocumentKind::Scheme));
        state.push_pending(pending(workspace.id, orphan, SyncDocumentKind::Scheme));

        queue_account_switch_reseed(&mut state, &crdt, &workspace, replica, &HashSet::new());

        assert!(state.pending.iter().any(|edit| edit.document == indexed));
        assert!(state.pending.iter().all(|edit| edit.document != orphan));
    }

    #[test]
    fn workspace_bootstrap_consumes_full_reseed_obligation_after_queueing_snapshots() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Indexed", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let mut crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        let mut state = LocalSyncState {
            reseed_all_documents: true,
            ..LocalSyncState::default()
        };

        let _ = queue_workspace_bootstrap_updates(
            &mut state,
            &mut crdt,
            &workspace,
            ReplicaId::new(),
            &HashMap::new(),
        );

        assert!(!state.needs_full_reseed());
        assert!(state
            .pending
            .iter()
            .any(|edit| { edit.document == workspace.scheme_sync[&scheme_id].id }));
    }

    #[test]
    fn routine_bootstrap_does_not_snapshot_documents_with_a_server_base() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Already synced", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let mut crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        let mut state = LocalSyncState::default();
        let remote_latest = HashMap::from([
            (workspace.sync.id, 8),
            (workspace.scheme_sync[&scheme_id].id, 8),
        ]);

        let healed = queue_workspace_bootstrap_updates(
            &mut state,
            &mut crdt,
            &workspace,
            ReplicaId::new(),
            &remote_latest,
        );

        assert!(healed.is_empty());
        assert!(state.pending.is_empty());
    }
}

#[cfg(test)]
mod compaction_tests {
    use super::{
        compact_pending_documents, LocalSyncState, PendingCrdtEdit, MAX_PENDING_PER_DOCUMENT,
    };
    use chrono::Utc;
    use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, WorkspaceId};
    use yrs::updates::decoder::Decode;
    use yrs::{Doc, GetString, ReadTxn, Text, Transact, Update};

    /// A real Yjs delta: append `text` to the doc and encode just that change.
    fn text_delta(doc: &Doc, text: &str) -> Vec<u8> {
        let before = doc.transact().state_vector();
        let root = doc.get_or_insert_text("body");
        {
            let mut txn = doc.transact_mut();
            let len = root.len(&txn);
            root.insert(&mut txn, len, text);
        }
        doc.transact().encode_diff_v1(&before)
    }

    fn edit(
        workspace: WorkspaceId,
        document: DocumentId,
        sequence: u64,
        update_v1: Vec<u8>,
    ) -> PendingCrdtEdit {
        PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace,
            replica_id: ReplicaId::new(),
            local_sequence: sequence,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1,
            touched_items: vec![format!("item-{sequence}")],
        }
    }

    /// Queue `n` real deltas for `document`, returning the text they build up.
    fn queue_deltas(
        state: &mut LocalSyncState,
        workspace: WorkspaceId,
        document: DocumentId,
        n: u64,
    ) -> String {
        let doc = Doc::new();
        let mut expected = String::new();
        for sequence in 1..=n {
            let piece = format!("{sequence} ");
            expected.push_str(&piece);
            let delta = text_delta(&doc, &piece);
            state.push_pending(edit(workspace, document, sequence, delta));
        }
        expected
    }

    fn apply_all(updates: impl IntoIterator<Item = Vec<u8>>) -> String {
        let doc = Doc::new();
        let root = doc.get_or_insert_text("body");
        for update in updates {
            let mut txn = doc.transact_mut();
            txn.apply_update(Update::decode_v1(&update).unwrap())
                .unwrap();
        }
        let txn = doc.transact();
        root.get_string(&txn)
    }

    #[test]
    fn a_short_queue_is_left_alone() {
        let workspace = WorkspaceId::new();
        let document = DocumentId::new();
        let mut state = LocalSyncState::default();
        queue_deltas(&mut state, workspace, document, 4);

        assert_eq!(
            compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT),
            0
        );
        assert_eq!(
            state.pending.len(),
            4,
            "nothing to gain from compacting yet"
        );
    }

    /// The point of the whole exercise: an unsyncable queue stops growing, and
    /// the one entry left behind still says everything the originals said.
    #[test]
    fn an_overfull_queue_merges_without_losing_anything() {
        let workspace = WorkspaceId::new();
        let document = DocumentId::new();
        let mut state = LocalSyncState::default();
        let expected = queue_deltas(&mut state, workspace, document, 40);

        assert_eq!(
            compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT),
            1
        );
        let remaining: Vec<&PendingCrdtEdit> = state
            .pending
            .iter()
            .filter(|e| e.document == document)
            .collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            apply_all([remaining[0].update_v1.clone()]),
            expected,
            "the merged update must reproduce exactly what the 40 deltas built"
        );
        assert_eq!(
            remaining[0].touched_items.len(),
            40,
            "touched items carry the union, for the epoch adoption rescue"
        );
    }

    /// Compaction must not touch a document still within its budget — each is
    /// bounded on its own.
    #[test]
    fn other_documents_keep_their_queued_deltas() {
        let workspace = WorkspaceId::new();
        let (busy, quiet) = (DocumentId::new(), DocumentId::new());
        let mut state = LocalSyncState::default();
        queue_deltas(&mut state, workspace, busy, 40);
        let quiet_text = queue_deltas(&mut state, workspace, quiet, 3);

        compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT);
        let kept: Vec<Vec<u8>> = state
            .pending
            .iter()
            .filter(|e| e.document == quiet)
            .map(|e| e.update_v1.clone())
            .collect();
        assert_eq!(kept.len(), 3);
        assert_eq!(apply_all(kept), quiet_text);
    }

    #[test]
    fn the_queue_stays_ordered_by_local_sequence() {
        let workspace = WorkspaceId::new();
        let (busy, other) = (DocumentId::new(), DocumentId::new());
        let mut state = LocalSyncState::default();
        queue_deltas(&mut state, workspace, busy, 40);
        state.push_pending(edit(workspace, other, 41, vec![0, 0]));

        compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT);
        let sequences: Vec<u64> = state.pending.iter().map(|e| e.local_sequence).collect();
        let mut sorted = sequences.clone();
        sorted.sort_unstable();
        assert_eq!(sequences, sorted, "push order must follow local_sequence");
    }

    /// Stable: otherwise every later edit would pay to merge again.
    #[test]
    fn compacting_again_is_a_no_op() {
        let workspace = WorkspaceId::new();
        let document = DocumentId::new();
        let mut state = LocalSyncState::default();
        queue_deltas(&mut state, workspace, document, 40);

        compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT);
        let after_first = state.pending.len();
        assert_eq!(
            compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT),
            0
        );
        assert_eq!(state.pending.len(), after_first);
    }

    /// Undecodable bytes must be left alone rather than thrown away — a queue we
    /// cannot compact is slow, but discarding it would lose edits.
    #[test]
    fn a_document_that_cannot_be_merged_is_left_intact() {
        let workspace = WorkspaceId::new();
        let document = DocumentId::new();
        let mut state = LocalSyncState::default();
        for sequence in 1..=40 {
            state.push_pending(edit(workspace, document, sequence, vec![0xff, 0xff, 0xff]));
        }

        assert_eq!(
            compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT),
            0
        );
        assert_eq!(state.pending.len(), 40);
    }
}

#[cfg(test)]
mod fold_pending_tests {
    use super::{fold_pending_edits_into_state, PendingCrdtEdit};
    use chrono::Utc;
    use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, WorkspaceId};
    use yrs::updates::decoder::Decode;
    use yrs::{Doc, Map, ReadTxn, StateVector, Transact, Update};

    fn state_of(doc: &Doc) -> Vec<u8> {
        doc.transact()
            .encode_state_as_update_v1(&StateVector::default())
    }

    fn restore(state: &[u8]) -> Doc {
        let doc = Doc::new();
        doc.transact_mut()
            .apply_update(Update::decode_v1(state).unwrap())
            .unwrap();
        doc
    }

    /// Set `value` on the doc and encode just that change.
    fn set(doc: &Doc, value: &str) -> Vec<u8> {
        let before = doc.transact().state_vector();
        let map = doc.get_or_insert_map("item");
        map.insert(&mut doc.transact_mut(), "text", value);
        doc.transact().encode_diff_v1(&before)
    }

    fn value(doc: &Doc) -> String {
        let map = doc.get_or_insert_map("item");
        let txn = doc.transact();
        map.get(&txn, "text").unwrap().to_string(&txn)
    }

    fn edit(document: DocumentId, sequence: u64, update_v1: Vec<u8>) -> PendingCrdtEdit {
        PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: WorkspaceId::new(),
            replica_id: ReplicaId::new(),
            local_sequence: sequence,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1,
            touched_items: Vec::new(),
        }
    }

    /// The saved state lacks a queued edit (a crash between the queue and CRDT
    /// saves). Restoring without the fold authors the next edit beside the
    /// queued one, which can then win; with the fold it always follows it.
    #[test]
    fn an_edit_after_restoring_follows_the_queued_edit_the_saved_state_missed() {
        let document = DocumentId::new();
        for _ in 0..32 {
            let session = Doc::new();
            set(&session, "saved");
            let saved = state_of(&session);
            let queued = set(&session, "queued");
            let pending = [edit(document, 1, queued.clone())];

            let folded = fold_pending_edits_into_state(document, &saved, &pending)
                .expect("the saved state missed the queued edit");
            let relaunched = restore(&folded);
            assert_eq!(value(&relaunched), "queued");
            let after = set(&relaunched, "after relaunch");

            // The queue is pushed alongside the new edit, in either order.
            for order in [[&queued, &after], [&after, &queued]] {
                let merged = restore(&folded);
                for update in order {
                    merged
                        .transact_mut()
                        .apply_update(Update::decode_v1(update).unwrap())
                        .unwrap();
                }
                assert_eq!(value(&merged), "after relaunch");
            }
        }
    }

    #[test]
    fn folding_changes_nothing_the_saved_state_already_holds() {
        let document = DocumentId::new();
        let session = Doc::new();
        let first = set(&session, "first");
        let second = set(&session, "second");
        let saved = state_of(&session);
        let pending = [
            edit(document, 2, second),
            edit(document, 1, first),
            // Another document's edit and an unreadable one are ignored.
            edit(DocumentId::new(), 3, set(&Doc::new(), "elsewhere")),
            edit(document, 4, vec![0xff, 0x00, 0x13]),
        ];
        assert_eq!(
            fold_pending_edits_into_state(document, &saved, &pending),
            None
        );
        // A document with no saved state is left for the sync that seeds it.
        assert_eq!(fold_pending_edits_into_state(document, &[], &pending), None);
    }
}
