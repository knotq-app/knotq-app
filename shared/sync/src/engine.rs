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
use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, Workspace, WorkspaceId};

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
        };
        let response = transport.pull(&request)?;
        if let Some(known_documents) = &response.known_documents {
            authoritative_remote_latest = Some(known_documents.clone());
        }
        if response.documents.is_empty() {
            break;
        }
        let workspace_id = workspace.id;
        let updates: Vec<StoredCrdtUpdate> = response
            .documents
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
            local_state.mark_pulled(doc.document, doc.kind, doc.seq);

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
                // Check if this is a crdt_schema_invalid rejection we can self-heal.
                let is_schema_invalid = err
                    .downcast_ref::<SyncPushRejected>()
                    .is_some_and(|e| e.code == "crdt_schema_invalid")
                    || err.to_string().contains("crdt_schema_invalid");

                if !is_schema_invalid {
                    return Err(err);
                }

                // Identify which documents were in the rejected batch and haven't been
                // reseeded yet.  For each, drop all pending edits and re-queue a full
                // snapshot from the live CRDT so the server can re-converge.
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
                        // Already reseeded this document — give up.
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
    for (document, kind) in distinct_pending_documents(local_state)
        .into_iter()
        .take(PUSH_MAX_DOCUMENTS_PER_REQUEST)
    {
        let edits = local_state.pending_for_document(document, PUSH_MAX_UPDATES_PER_DOCUMENT);
        let through = edits.iter().map(|edit| edit.local_sequence).max();
        let Some(through) = through else {
            continue;
        };
        max_through = max_through.max(through);
        let sent_edits: Vec<(OperationId, u64)> = edits
            .iter()
            .map(|e| (e.operation_id, e.local_sequence))
            .collect();
        documents.push(PushDocumentUpdates {
            document,
            kind,
            updates: edits.into_iter().map(|edit| edit.update_v1).collect(),
        });
        acks.push(DocumentAck {
            document,
            through_local_sequence: through,
            sent_edits,
        });
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
