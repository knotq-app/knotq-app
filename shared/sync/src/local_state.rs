use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, Workspace, WorkspaceId};
use serde::{Deserialize, Serialize};
use yrs::updates::{decoder::Decode, encoder::Encode};
use yrs::Update;

use crate::{
    validate_crdt_update_sequence, CrdtDocumentUpdate, PushUpdatesRequest, SyncDocumentRef,
    WorkspaceCrdtDocuments, SYNC_STATE_RECOVERY_VERSION,
};

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
    #[serde(default)]
    pub document_cursors: HashMap<DocumentId, DocumentSyncCursor>,
    #[serde(default)]
    pub media_cursors: HashMap<String, MediaSyncCursor>,
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
        self.pending
            .retain(|edit| edit.kind != SyncDocumentKind::PersonalWorkspace);
    }

    /// As [`clear_cursors_for_full_repull`], plus arming the full-snapshot
    /// re-seed an account/server change requires (see `reseed_all_documents`).
    fn clear_cursors_for_account_change(&mut self) {
        self.clear_cursors_for_full_repull();
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
    let healed = crdt.heal_schema_invalid_documents(workspace, |document| {
        remote_latest.get(&document).copied().unwrap_or(0) == 0
    });
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
    let reseed_all = sync_state.needs_full_reseed();
    for update in crdt.full_snapshot_updates().updates {
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
/// rather than duplicate. Call on a detected account switch, before the pull; the
/// bootstrap then treats these queued snapshots as valid pending and does not re-queue.
pub fn queue_account_switch_reseed(
    sync_state: &mut LocalSyncState,
    crdt: &WorkspaceCrdtDocuments,
    workspace: &Workspace,
    replica_id: ReplicaId,
) {
    let mut next_sequence = sync_state
        .pending
        .iter()
        .map(|edit| edit.local_sequence)
        .max()
        .unwrap_or(0)
        + 1;
    for update in crdt.full_snapshot_updates().updates {
        if update.kind != SyncDocumentKind::Scheme {
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
    use super::{DocumentSyncCursor, LocalSyncState, MediaSyncCursor, PendingCrdtEdit};
    use chrono::Utc;
    use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, WorkspaceId};

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

        assert!(state.reset_for_account_change(account_b, SERVER_A));

        assert!(
            state.document_cursors.is_empty(),
            "pull/push cursors cleared"
        );
        assert!(state.media_cursors.is_empty(), "media cursors cleared");
        // Scheme content pending is kept; workspace-index pending is dropped.
        assert_eq!(state.pending.len(), 1);
        assert!(state
            .pending
            .iter()
            .all(|edit| edit.kind == SyncDocumentKind::Scheme));
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
}

#[cfg(test)]
mod compaction_tests {
    use super::{compact_pending_documents, LocalSyncState, PendingCrdtEdit, MAX_PENDING_PER_DOCUMENT};
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
            txn.apply_update(Update::decode_v1(&update).unwrap()).unwrap();
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
        assert_eq!(state.pending.len(), 4, "nothing to gain from compacting yet");
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
        assert_eq!(compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT), 0);
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

        assert_eq!(compact_pending_documents(&mut state, MAX_PENDING_PER_DOCUMENT), 0);
        assert_eq!(state.pending.len(), 40);
    }
}
