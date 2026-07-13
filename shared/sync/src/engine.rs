//! Platform-independent batched sync engine.
//!
//! Both the desktop (`sync_service.rs`) and the iOS/Android core (`mobile/core`)
//! drive sync through this engine, so the wire protocol and CRDT merge logic live
//! in exactly one place. Each platform supplies a [`SyncTransport`] (its own HTTP
//! client) and keeps its own platform I/O — workspace load/save, media upload, and
//! scheduling — around the two engine entry points.
//!
//! The engine speaks the merged-state batched protocol: [`batch_pull_and_apply`]
//! fetches the whole workspace in one (paged) request and applies each changed
//! document's merged state, and [`batch_push_pending`] sends every dirty document
//! in as few requests as the bounds allow.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use chrono::Utc;
use knotq_model::{
    DocumentId, OperationId, ReplicaId, SyncDocumentKind, Workspace, WorkspaceId,
};

use crate::{
    BatchPullRequest, BatchPushRequest, LocalSyncState, NotificationScheduleSnapshot,
    PendingCrdtEdit, PulledCrdtDocument, PushDocumentUpdates, StoredCrdtUpdate,
    WorkspaceCrdtDocuments,
};

/// A document that was included in a pull response but could not be applied
/// locally. Its pull cursor was still advanced so we do not re-fetch it every
/// cycle (the merged-state protocol guarantees a future update will re-deliver
/// the full merged state when the document changes). The caller may log or
/// surface these for diagnostics; they are never fatal to the pull.
#[derive(Clone, Debug)]
pub struct SkippedDocument {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    /// True when the skip is benign: the document is not in the local workspace
    /// index (orphan or deleted-scheme content doc). Callers can suppress noisy
    /// logging for these — they are expected in normal operation.
    pub unknown_scheme_document: bool,
    pub reason: String,
}

/// Upper bound on documents the client packs into one batched push. Comfortably
/// under the server's `MAX_SYNC_PUSH_DOCUMENTS`; remaining dirty documents go in the
/// next request inside [`batch_push_pending`]'s loop.
pub const PUSH_MAX_DOCUMENTS_PER_REQUEST: usize = 64;
/// Per-document update cap; matches the server's `MAX_CRDT_UPDATES_PER_PUSH`.
pub const PUSH_MAX_UPDATES_PER_DOCUMENT: usize = 50;
/// Soft cap on raw CRDT update bytes in one push request. The wire body base64
/// expands these bytes, so this stays below the backend JSON body cap; if one
/// individual update exceeds the cap we still send it alone and let the backend's
/// per-update limit decide.
pub const PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST: usize = 6 * 1024 * 1024;

/// The transport a platform implements to carry batched sync requests. Calls are
/// synchronous and may block (the drivers run them off the UI thread). Tests supply
/// an in-memory implementation, so the engine never depends on real networking.
pub trait SyncTransport {
    fn pull(&self, request: &BatchPullRequest) -> Result<crate::BatchPullResponse>;
    fn push(&self, request: &BatchPushRequest) -> Result<crate::BatchPushResponse>;
}

/// Typed error returned (wrapped in `anyhow::Error`) when the server rejects a push
/// with a 4xx status code. The `code` field carries the machine-readable error code
/// from the backend (e.g. `"crdt_schema_invalid"`).
#[derive(Debug)]
pub struct SyncPushRejected {
    pub code: String,
}

impl std::fmt::Display for SyncPushRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sync backend rejected request: {}", self.code)
    }
}

impl std::error::Error for SyncPushRejected {}

/// The server rejection code for updates authored against a stale document epoch.
pub const SYNC_PUSH_EPOCH_STALE_CODE: &str = "document_epoch_stale";

/// Typed error surfaced when a push is rejected as `document_epoch_stale`: some
/// document was squashed since this replica last pulled. This is NOT healed by
/// the reseed below (a reseeded snapshot still carries the stale epoch) — the
/// driver must run one more pull-then-push cycle: the pull adopts the squashed
/// state and re-expresses the pending edits against it, after which the push
/// succeeds. Bounded like the driver's unauthorized retry: once per sync run.
#[derive(Debug)]
pub struct SyncPushEpochStale;

impl std::fmt::Display for SyncPushEpochStale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "sync push rejected: document epoch stale (re-pull required)"
        )
    }
}

impl std::error::Error for SyncPushEpochStale {}

/// Result of [`batch_pull_and_apply`]: the workspace after merging remote state and
/// how many remote document states were applied.
pub struct PullOutcome {
    pub workspace: Workspace,
    pub remote_updates_applied: usize,
    pub remote_latest: HashMap<DocumentId, u64>,
    /// Documents that arrived in the pull response but could not be applied
    /// locally. Their cursors were advanced anyway — see [`SkippedDocument`].
    pub skipped: Vec<SkippedDocument>,
}

/// A document whose pending edits the server accepted, with the local sequence the
/// push covered, so the caller can clear those pending edits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PushedDocument {
    pub document: DocumentId,
    pub through_local_sequence: u64,
}

/// Pull the whole workspace and apply every changed document's merged state.
///
/// Sends the client's per-document cursors in one request; the server replies with
/// the merged `state_v1` for every document whose `seq` advanced (and, for a
/// zero/absent cursor, documents created on other devices). Applying merged state
/// is idempotent in Yjs, so it converges over any local pending edits. Follows the
/// server's `has_more` flag, re-pulling with advanced cursors until caught up.
pub fn batch_pull_and_apply(
    transport: &dyn SyncTransport,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    local_state: &mut LocalSyncState,
    workspace: Workspace,
    replica_id: ReplicaId,
) -> Result<PullOutcome> {
    let mut workspace = workspace;
    let mut remote_updates_applied = 0;
    let mut authoritative_remote_latest: Option<HashMap<DocumentId, u64>> = None;
    let mut all_skipped: Vec<SkippedDocument> = Vec::new();
    loop {
        let request = BatchPullRequest {
            replica_id,
            cursors: local_state
                .document_cursors
                .values()
                .map(|cursor| (cursor.document, cursor.last_pulled_sequence))
                .collect(),
            client_protocol_version: crate::CLIENT_SYNC_PROTOCOL_VERSION,
        };
        let response = transport.pull(&request)?;
        if let Some(known_documents) = &response.known_documents {
            authoritative_remote_latest = Some(known_documents.clone());
        }
        if response.documents.is_empty() {
            break;
        }
        let workspace_id = workspace.id;

        // Partition out epoch adoptions: a scheme document whose epoch differs
        // from the one this replica last recorded was squashed (its state shares
        // no Yjs history with the local document), so it must be REPLACED, not
        // merged — a merge would double every item's text. Only documents with
        // an existing cursor qualify; a first-ever pull merges into an empty
        // local document, which is already an exact copy.
        let needs_adoption = |doc: &PulledCrdtDocument| {
            doc.kind == SyncDocumentKind::Scheme
                && local_state
                    .document_cursors
                    .get(&doc.document)
                    .is_some_and(|cursor| cursor.epoch != doc.epoch)
        };
        let (adoptions, merges): (Vec<&PulledCrdtDocument>, Vec<&PulledCrdtDocument>) = response
            .documents
            .iter()
            .partition(|doc| needs_adoption(doc));

        let updates: Vec<StoredCrdtUpdate> = merges
            .iter()
            .map(|doc| pulled_document_as_update(workspace_id, doc))
            .collect();
        // `apply_remote_updates` applies workspace-kind updates (and re-materializes)
        // before scheme-kind ones, so a scheme created on another device — whose
        // workspace-index entry and scheme document arrive in the same response — is
        // routed correctly even though this replica had never seen it.
        let outcome = crdt_docs.apply_remote_updates(&workspace, &updates);
        // Workspace-level errors (corrupt index, materialization failure) are fatal:
        // we cannot trust the resulting workspace or any scheme content.
        if !outcome.workspace_is_ok() {
            return Err(anyhow!(
                "CRDT workspace apply failed: {:?}",
                outcome
                    .workspace_errors
                    .iter()
                    .map(|e| e.message.as_str())
                    .collect::<Vec<_>>()
            ));
        }
        remote_updates_applied += outcome.applied;
        workspace = outcome.workspace;

        // Apply the epoch adoptions AFTER the merged updates, so a workspace-
        // index update arriving in the same response has already registered the
        // scheme (the adoption resolves the scheme through the workspace index).
        for doc in adoptions {
            let touched = local_state
                .has_pending_for_document(doc.document)
                .then(|| local_state.pending_touched_items(doc.document));
            match crdt_docs.adopt_squashed_document(
                &workspace,
                doc.document,
                &doc.state_v1,
                touched.as_ref(),
            ) {
                Ok((adopted_workspace, rescue)) => {
                    workspace = adopted_workspace;
                    remote_updates_applied += 1;
                    // The old pending deltas are unusable against the adopted
                    // document (stale epoch); the rescue re-expresses them.
                    local_state
                        .pending
                        .retain(|edit| edit.document != doc.document);
                    if let Some(rescue) = rescue {
                        let next_sequence = local_state
                            .pending
                            .iter()
                            .map(|edit| edit.local_sequence)
                            .max()
                            .unwrap_or(0)
                            + 1;
                        local_state.push_pending(PendingCrdtEdit {
                            operation_id: OperationId::new(),
                            workspace_id,
                            replica_id,
                            local_sequence: next_sequence,
                            created_at: Utc::now(),
                            document: rescue.document,
                            kind: rescue.kind,
                            update_v1: rescue.update_v1,
                            touched_items: rescue.touched_items,
                        });
                    }
                }
                Err(err) => {
                    // Mirrors the merge path's skip semantics: advance the
                    // cursor (below) — the server re-delivers full state on the
                    // next bump — and surface the document for diagnostics. An
                    // unknown scheme (deleted on another device) is benign.
                    let unknown = !scheme_document_known(&workspace, doc.document);
                    all_skipped.push(SkippedDocument {
                        document: doc.document,
                        kind: doc.kind,
                        unknown_scheme_document: unknown,
                        reason: format!("epoch adoption: {err:#}"),
                    });
                }
            }
        }

        // Build a set of document ids that had per-document errors so we can
        // still advance their cursors (the server will re-deliver full merged
        // state on the next bump; we do not want to re-pull indefinitely).
        let errored_document_ids: HashMap<DocumentId, &crate::DocumentApplyError> = outcome
            .document_errors
            .iter()
            .map(|e| (e.document, e))
            .collect();

        for doc in &response.documents {
            // Always advance the cursor — including for skipped documents.
            // Advancing past a failed document is safe because the merged-state
            // protocol re-delivers the *full* merged state whenever the server
            // sequence advances, so we lose nothing permanently. We only skip
            // our local application; the content is still on the server and
            // will be re-pulled the next time that document is touched.
            local_state.mark_pulled(doc.document, doc.kind, doc.seq, doc.epoch);

            if let Some(err) = errored_document_ids.get(&doc.document) {
                all_skipped.push(SkippedDocument {
                    document: doc.document,
                    kind: doc.kind,
                    unknown_scheme_document: err.unknown_scheme_document,
                    reason: err.message.clone(),
                });
            }
        }

        // Re-convergence: after applying workspace updates, any scheme that is
        // now in the workspace index but whose local CRDT doc is missing (or was
        // in this pull's skipped set) needs its pull cursor reset to 0 so the
        // next poll fetches its full merged state from sequence zero. This is
        // safe — we only reset, we never loop within this call — and ensures
        // that an orphan-then-index-added sequence eventually converges.
        let skipped_document_ids: std::collections::HashSet<DocumentId> =
            errored_document_ids.keys().copied().collect();
        let local_crdt_doc_ids = crdt_docs.known_document_ids();
        for (scheme_id, meta) in &workspace.scheme_sync {
            if meta.kind != SyncDocumentKind::Scheme {
                continue;
            }
            let missing_locally = !local_crdt_doc_ids.contains(&meta.id);
            let was_skipped = skipped_document_ids.contains(&meta.id);
            if missing_locally || was_skipped {
                // Reset so the next poll re-pulls from seq 0 for this document.
                local_state.reset_pull_cursor(meta.id);
            }
            let _ = scheme_id; // used via meta
        }

        if !response.has_more {
            break;
        }
    }
    let remote_latest = authoritative_remote_latest.unwrap_or_else(|| {
        local_state
            .document_cursors
            .values()
            .map(|cursor| (cursor.document, cursor.last_pulled_sequence))
            .collect()
    });
    Ok(PullOutcome {
        workspace,
        remote_updates_applied,
        remote_latest,
        skipped: all_skipped,
    })
}

/// Push every dirty document in as few batched requests as the bounds allow,
/// clearing pending edits the server accepts. Accepted documents are appended to
/// `pushed` and removed from `local_state.pending` as each request returns, so a
/// later request failing still leaves earlier progress recorded — the caller can
/// persist `local_state` and clear the already-pushed edits before propagating the
/// error (mirroring the durable-cursor-on-partial-failure contract).
///
/// When the server returns `crdt_schema_invalid`, the engine self-heals: it drops
/// the bad pending edits for affected documents and re-queues a full snapshot from
/// `crdt_docs`, then retries once.  Each document is reseeded at most once per call
/// — a second rejection for a reseeded document is returned as an error.
pub fn batch_push_pending(
    transport: &dyn SyncTransport,
    local_state: &mut LocalSyncState,
    replica_id: ReplicaId,
    notification_schedule: &NotificationScheduleSnapshot,
    pushed: &mut Vec<PushedDocument>,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    workspace: &Workspace,
) -> Result<()> {
    // Track which documents we've already reseeded this call; a second rejection
    // after reseed means something is deeply wrong — propagate that error.
    let mut reseeded: HashSet<DocumentId> = HashSet::new();
    loop {
        let Some((request, acks)) =
            build_push_request(local_state, replica_id, notification_schedule)
        else {
            return Ok(());
        };
        let push_result = transport.push(&request);
        match push_result {
            Ok(response) => {
                let accepted_by_document: HashMap<DocumentId, usize> = response
                    .documents
                    .iter()
                    .map(|doc| (doc.document, doc.accepted))
                    .collect();
                for (sent, ack) in request.documents.iter().zip(acks.iter()) {
                    let accepted = accepted_by_document.get(&ack.document).copied();
                    if accepted != Some(sent.updates.len()) {
                        return Err(anyhow!(
                            "sync backend accepted {:?}/{} updates for {}",
                            accepted,
                            sent.updates.len(),
                            ack.document
                        ));
                    }
                    // Use exact edit-ID clearing so duplicate-sequence edits from a
                    // legacy restart are not silently dropped.
                    local_state.mark_pushed_edits(ack.document, &ack.sent_edits);
                    pushed.push(PushedDocument {
                        document: ack.document,
                        through_local_sequence: ack.through_local_sequence,
                    });
                }
            }
            Err(err) => {
                // Only a deterministic server rejection (an HTTP 4xx carrying a
                // `SyncPushRejected` code) is safe to self-heal. A transport/network
                // error is transient, so abort and let the next sync retry the whole
                // batch — the caller has already persisted the pull cursors, so no
                // pull progress is lost. Previously only `crdt_schema_invalid` was
                // healed and every other rejection code (e.g. `updates_too_large`,
                // `update_payload_invalid`) aborted the sync permanently; reseeding
                // for any rejection lets the merged snapshot recover cases a single
                // bad delta could not.
                let Some(rejection) = err.downcast_ref::<SyncPushRejected>() else {
                    return Err(err);
                };
                // A stale document epoch is NOT healable by reseeding — the
                // reseeded snapshot still carries the old epoch and would be
                // rejected identically, wedging the run. Surface the typed
                // error so the driver runs one more pull (which adopts the
                // squashed state and re-expresses pending edits) and retries.
                if rejection.code == SYNC_PUSH_EPOCH_STALE_CODE {
                    return Err(err.context(SyncPushEpochStale));
                }

                // Identify which documents were in the rejected batch and haven't been
                // reseeded yet.  For each, drop all pending edits and re-queue a full
                // snapshot from the live CRDT so the server can re-converge. A reseed
                // never loses data: the snapshot is regenerated from the on-disk CRDT.
                let next_seq = local_state
                    .pending
                    .iter()
                    .map(|e| e.local_sequence)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let mut seq = next_seq;
                let mut any_reseeded = false;
                // A reseed only helps if the snapshot itself validates. Repair any
                // rejected document whose local doc is schema-less (e.g. a scheme
                // created by a direct workspace mutation that never reached the
                // CRDT) before snapshotting it.
                let rejected: HashSet<DocumentId> = acks.iter().map(|a| a.document).collect();
                for document in
                    crdt_docs.heal_schema_invalid_documents(workspace, |id| rejected.contains(&id))
                {
                    eprintln!("sync push self-heal: repopulated schema-less document {document}");
                }
                for ack in &acks {
                    if reseeded.contains(&ack.document) {
                        // Already reseeded this document and it still rejected — the
                        // server-side batch rejection is all-or-nothing, so we cannot
                        // tell which document is at fault. Give up rather than
                        // quarantine (dropping a sibling document batched with it
                        // would silently lose its edits); the pull cursors are
                        // already durable, so this surfaces as a retryable error
                        // without losing pull progress or local data.
                        return Err(err);
                    }
                    reseeded.insert(ack.document);
                    any_reseeded = true;

                    // Drop all pending edits for this document.
                    local_state.pending.retain(|e| e.document != ack.document);

                    // Re-queue a full snapshot from the persistent CRDT documents so
                    // the reseed shares identity (clientID + clocks) with this
                    // device's incremental diffs — same rationale as
                    // queue_workspace_bootstrap_updates.
                    let snapshot_updates = crdt_docs.full_snapshot_updates();
                    for update in snapshot_updates.updates {
                        if update.document != ack.document {
                            continue;
                        }
                        local_state.push_pending(PendingCrdtEdit {
                            operation_id: OperationId::new(),
                            workspace_id: workspace.id,
                            replica_id,
                            local_sequence: seq,
                            created_at: Utc::now(),
                            document: update.document,
                            kind: update.kind,
                            update_v1: update.update_v1,
                            touched_items: update.touched_items,
                        });
                        seq += 1;
                    }
                }

                if !any_reseeded {
                    return Err(err);
                }
                // Loop continues — retry with the reseeded snapshot.
            }
        }
    }
}

/// A scheme document's state must exceed this before a squash is proposed —
/// below it, the history overhead simply doesn't matter.
pub const SQUASH_MIN_STATE_BYTES: usize = 256 * 1024;
/// ... and the history-free rebuild must be at least this many times smaller,
/// so a large document that is genuinely mostly content is left alone.
pub const SQUASH_MIN_RATIO: usize = 4;

/// A candidate history squash: the rebuilt state plus the compare-and-set base
/// the server verifies. Built only from a fully-synced document; the driver
/// POSTs it to `/v1/sync/squash` and treats every rejection as a benign skip.
#[derive(Clone, Debug)]
pub struct SquashProposal {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    pub base_epoch: u64,
    pub base_seq: u64,
    pub state_v1: Vec<u8>,
    pub bytes_before: usize,
}

impl SquashProposal {
    pub fn as_request(&self, replica_id: ReplicaId) -> crate::SquashDocumentRequest {
        crate::SquashDocumentRequest {
            replica_id,
            document: self.document,
            kind: self.kind,
            base_epoch: self.base_epoch,
            base_seq: self.base_seq,
            state_v1: self.state_v1.clone(),
            client_protocol_version: crate::CLIENT_SYNC_PROTOCOL_VERSION,
        }
    }
}

/// Propose at most ONE history squash, choosing the largest eligible scheme
/// document. Eligibility is strictly conservative — the document must be fully
/// synced from this replica's point of view:
///   - no pending local edits for it (its content equals what was pushed), and
///   - a non-zero pull cursor (the base_seq compare-and-set value); if another
///     device pushed since, the server head moved and the squash is rejected
///     as a harmless `squash_conflict`.
/// Size gates keep this from ever firing on healthy documents. Returns `None`
/// when nothing qualifies — the common case.
pub fn build_squash_proposal(
    crdt_docs: &WorkspaceCrdtDocuments,
    local_state: &LocalSyncState,
) -> Option<SquashProposal> {
    for (document, state_len) in crdt_docs.squash_candidates(SQUASH_MIN_STATE_BYTES) {
        if local_state.has_pending_for_document(document) {
            continue;
        }
        let Some(cursor) = local_state.document_cursors.get(&document) else {
            continue;
        };
        if cursor.last_pulled_sequence == 0 || cursor.kind != SyncDocumentKind::Scheme {
            continue;
        }
        let Ok(state_v1) = crdt_docs.rebuild_scheme_state(document) else {
            continue;
        };
        if state_v1.len().saturating_mul(SQUASH_MIN_RATIO) > state_len {
            continue;
        }
        return Some(SquashProposal {
            document,
            kind: SyncDocumentKind::Scheme,
            base_epoch: cursor.epoch,
            base_seq: cursor.last_pulled_sequence,
            state_v1,
            bytes_before: state_len,
        });
    }
    None
}

// Per-document ack that includes the exact (operation_id, local_sequence) pairs
// that were sent, so `mark_pushed_edits` can clear precisely those entries.
struct DocumentAck {
    document: DocumentId,
    through_local_sequence: u64,
    sent_edits: Vec<(knotq_model::OperationId, u64)>,
}

// Build one batched push request from the head of the pending queue, plus the acks
// the caller applies once the server confirms acceptance. Returns `None` when there
// is nothing pending. Each iteration of the caller's loop removes the documents it
// covered (via `mark_pushed_edits`), so the queue strictly shrinks and the loop ends.
fn build_push_request(
    local_state: &LocalSyncState,
    fallback_replica_id: ReplicaId,
    notification_schedule: &NotificationScheduleSnapshot,
) -> Option<(BatchPushRequest, Vec<DocumentAck>)> {
    let mut documents = Vec::new();
    let mut acks = Vec::new();
    let mut max_through = 0;
    let mut raw_update_bytes = 0usize;
    for (document, kind) in distinct_pending_documents(local_state) {
        if documents.len() >= PUSH_MAX_DOCUMENTS_PER_REQUEST {
            break;
        }
        let candidates = local_state.pending_for_document(document, PUSH_MAX_UPDATES_PER_DOCUMENT);
        let candidate_count = candidates.len();
        let mut edits = Vec::new();
        for edit in candidates {
            let edit_len = edit.update_v1.len();
            let would_exceed =
                raw_update_bytes.saturating_add(edit_len) > PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST;
            if would_exceed && !(documents.is_empty() && edits.is_empty()) {
                break;
            }
            raw_update_bytes = raw_update_bytes.saturating_add(edit_len);
            edits.push(edit);
            if edit_len > PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST {
                break;
            }
        }
        let through = edits.iter().map(|edit| edit.local_sequence).max();
        let Some(through) = through else {
            break;
        };
        max_through = max_through.max(through);
        let sent_edits: Vec<(OperationId, u64)> = edits
            .iter()
            .map(|e| (e.operation_id, e.local_sequence))
            .collect();
        documents.push(PushDocumentUpdates {
            document,
            kind,
            epoch: local_state.document_epoch(document),
            updates: edits.into_iter().map(|edit| edit.update_v1).collect(),
        });
        acks.push(DocumentAck {
            document,
            through_local_sequence: through,
            sent_edits,
        });
        if acks
            .last()
            .is_some_and(|ack| ack.sent_edits.len() < candidate_count)
            || raw_update_bytes >= PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST
        {
            break;
        }
    }
    if documents.is_empty() {
        return None;
    }
    let mut schedule = notification_schedule.clone();
    schedule.sequence = max_through;
    Some((
        BatchPushRequest {
            replica_id: local_state.replica_id.unwrap_or(fallback_replica_id),
            documents,
            notification_schedule_changed: false,
            notification_schedule: Some(schedule),
            client_protocol_version: crate::CLIENT_SYNC_PROTOCOL_VERSION,
        },
        acks,
    ))
}

// Distinct documents present in the pending queue, in first-appearance order.
fn distinct_pending_documents(local_state: &LocalSyncState) -> Vec<(DocumentId, SyncDocumentKind)> {
    let mut seen = HashSet::new();
    let mut documents = Vec::new();
    for edit in &local_state.pending {
        if seen.insert(edit.document) {
            documents.push((edit.document, edit.kind));
        }
    }
    documents
}

fn scheme_document_known(workspace: &Workspace, document: DocumentId) -> bool {
    workspace
        .scheme_sync
        .values()
        .any(|meta| meta.id == document)
}

// Adapt a pulled merged-state document into the `StoredCrdtUpdate` shape
// `apply_remote_updates` consumes. Only `document`, `kind`, and `update_v1` (plus
// `sequence` for diagnostics) are read; the remaining fields are placeholders.
fn pulled_document_as_update(
    workspace_id: WorkspaceId,
    document: &PulledCrdtDocument,
) -> StoredCrdtUpdate {
    StoredCrdtUpdate {
        workspace_id,
        document: document.document,
        kind: document.kind,
        replica_id: ReplicaId::new(),
        sequence: document.seq,
        received_at: Utc::now(),
        update_v1: document.state_v1.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule() -> NotificationScheduleSnapshot {
        let now = Utc::now();
        NotificationScheduleSnapshot {
            sequence: 0,
            hash: "test-schedule".to_string(),
            window_start: now,
            window_end: now,
            occurrence_count: 0,
        }
    }

    fn pending(
        workspace_id: WorkspaceId,
        replica_id: ReplicaId,
        document: DocumentId,
        sequence: u64,
        byte_len: usize,
    ) -> PendingCrdtEdit {
        PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id,
            replica_id,
            local_sequence: sequence,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1: vec![sequence as u8; byte_len],
            touched_items: Vec::new(),
        }
    }

    fn raw_request_bytes(request: &BatchPushRequest) -> usize {
        request
            .documents
            .iter()
            .flat_map(|doc| doc.updates.iter())
            .map(Vec::len)
            .sum()
    }

    #[test]
    fn build_push_request_splits_hot_document_by_raw_bytes() {
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let document = DocumentId::new();
        let update_len = PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST / 2 + 1;
        let mut state = LocalSyncState {
            workspace_id: Some(workspace_id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.push_pending(pending(workspace_id, replica_id, document, 1, update_len));
        state.push_pending(pending(workspace_id, replica_id, document, 2, update_len));

        let (request, acks) = build_push_request(&state, replica_id, &schedule()).unwrap();

        assert_eq!(request.documents.len(), 1);
        assert_eq!(request.documents[0].document, document);
        assert_eq!(request.documents[0].updates.len(), 1);
        assert!(raw_request_bytes(&request) <= PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST);
        assert_eq!(acks[0].sent_edits.len(), 1);
        assert_eq!(acks[0].through_local_sequence, 1);
    }

    #[test]
    fn build_push_request_sends_single_oversized_update_alone() {
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let huge_document = DocumentId::new();
        let small_document = DocumentId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace_id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.push_pending(pending(
            workspace_id,
            replica_id,
            huge_document,
            1,
            PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST + 1,
        ));
        state.push_pending(pending(workspace_id, replica_id, small_document, 2, 8));

        let (request, acks) = build_push_request(&state, replica_id, &schedule()).unwrap();

        assert_eq!(request.documents.len(), 1);
        assert_eq!(request.documents[0].document, huge_document);
        assert_eq!(request.documents[0].updates.len(), 1);
        assert!(raw_request_bytes(&request) > PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST);
        assert_eq!(acks[0].sent_edits.len(), 1);
    }

    #[test]
    fn build_push_request_stops_before_next_document_would_exceed_raw_bytes() {
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let first = DocumentId::new();
        let second = DocumentId::new();
        let update_len = PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST / 2 + 1;
        let mut state = LocalSyncState {
            workspace_id: Some(workspace_id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.push_pending(pending(workspace_id, replica_id, first, 1, update_len));
        state.push_pending(pending(workspace_id, replica_id, second, 2, update_len));

        let (request, acks) = build_push_request(&state, replica_id, &schedule()).unwrap();

        assert_eq!(request.documents.len(), 1);
        assert_eq!(request.documents[0].document, first);
        assert!(raw_request_bytes(&request) <= PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST);
        assert_eq!(acks[0].through_local_sequence, 1);
    }
}
