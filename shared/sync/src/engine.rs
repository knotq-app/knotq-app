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
use knotq_model::{DocumentId, ReplicaId, SyncDocumentKind, Workspace, WorkspaceId};

use crate::{
    BatchPullRequest, BatchPushRequest, LocalSyncState, NotificationScheduleSnapshot,
    PulledCrdtDocument, PushDocumentUpdates, StoredCrdtUpdate, WorkspaceCrdtDocuments,
};

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

/// Result of [`batch_pull_and_apply`]: the workspace after merging remote state and
/// how many remote document states were applied.
pub struct PullOutcome {
    pub workspace: Workspace,
    pub remote_updates_applied: usize,
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
        if !outcome.is_ok() {
            return Err(anyhow!("CRDT apply failed: {:?}", outcome.errors));
        }
        remote_updates_applied += outcome.applied;
        workspace = outcome.workspace;
        for doc in &response.documents {
            local_state.mark_pulled(doc.document, doc.kind, doc.seq);
        }
        if !response.has_more {
            break;
        }
    }
    Ok(PullOutcome {
        workspace,
        remote_updates_applied,
    })
}

/// Push every dirty document in as few batched requests as the bounds allow,
/// clearing pending edits the server accepts. Accepted documents are appended to
/// `pushed` and removed from `local_state.pending` as each request returns, so a
/// later request failing still leaves earlier progress recorded — the caller can
/// persist `local_state` and clear the already-pushed edits before propagating the
/// error (mirroring the durable-cursor-on-partial-failure contract).
pub fn batch_push_pending(
    transport: &dyn SyncTransport,
    local_state: &mut LocalSyncState,
    replica_id: ReplicaId,
    notification_schedule: &NotificationScheduleSnapshot,
    pushed: &mut Vec<PushedDocument>,
) -> Result<()> {
    loop {
        let Some((request, acks)) = build_push_request(local_state, replica_id, notification_schedule)
        else {
            return Ok(());
        };
        let response = transport.push(&request)?;
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
            local_state.mark_pushed(ack.document, ack.through_local_sequence);
            pushed.push(*ack);
        }
    }
}

// Build one batched push request from the head of the pending queue, plus the acks
// the caller applies once the server confirms acceptance. Returns `None` when there
// is nothing pending. Each iteration of the caller's loop removes the documents it
// covered (via `mark_pushed`), so the queue strictly shrinks and the loop ends.
fn build_push_request(
    local_state: &LocalSyncState,
    fallback_replica_id: ReplicaId,
    notification_schedule: &NotificationScheduleSnapshot,
) -> Option<(BatchPushRequest, Vec<PushedDocument>)> {
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
        documents.push(PushDocumentUpdates {
            document,
            kind,
            updates: edits.into_iter().map(|edit| edit.update_v1).collect(),
        });
        acks.push(PushedDocument {
            document,
            through_local_sequence: through,
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
fn distinct_pending_documents(
    local_state: &LocalSyncState,
) -> Vec<(DocumentId, SyncDocumentKind)> {
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
