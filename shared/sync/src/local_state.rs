use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, Workspace, WorkspaceId};
use serde::{Deserialize, Serialize};

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
}

impl PendingCrdtEdit {
    pub fn as_update(&self) -> CrdtDocumentUpdate {
        CrdtDocumentUpdate {
            document: self.document,
            kind: self.kind,
            update_v1: self.update_v1.clone(),
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
        self.document_cursors.clear();
        self.media_cursors.clear();
        self.pending
            .retain(|edit| edit.kind != SyncDocumentKind::PersonalWorkspace);
        self.recovery_version = SYNC_STATE_RECOVERY_VERSION;
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
                });
            cursor.last_pushed_sequence = cursor.last_pushed_sequence.max(max_seq);
        }
    }

    pub fn mark_pulled(
        &mut self,
        document: DocumentId,
        kind: SyncDocumentKind,
        latest_sequence: u64,
    ) {
        let cursor = self
            .document_cursors
            .entry(document)
            .or_insert(DocumentSyncCursor {
                document,
                kind,
                last_pulled_sequence: 0,
                last_pushed_sequence: 0,
            });
        cursor.kind = kind;
        cursor.last_pulled_sequence = cursor.last_pulled_sequence.max(latest_sequence);
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
    for update in crdt.full_snapshot_updates().updates {
        if remote_latest.get(&update.document).copied().unwrap_or(0) != 0 {
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
