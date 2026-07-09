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
    /// Item ids this edit touched (see [`CrdtDocumentUpdate::touched_items`]).
    /// Defaults empty for edits persisted by pre-epoch builds; the adoption
    /// rescue treats such edits conservatively (every local item counts as
    /// touched, so nothing local is dropped).
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
        self.clear_cursors_for_full_repull();
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
    /// the adoption rescue. `None` when any pending edit predates touched-item
    /// tracking (persisted by an older build) — the caller must then treat
    /// every local item as touched rather than silently dropping local edits.
    pub fn pending_touched_items(&self, document: DocumentId) -> Option<HashSet<String>> {
        let mut touched = HashSet::new();
        for edit in self.pending.iter().filter(|e| e.document == document) {
            if edit.touched_items.is_empty() {
                return None;
            }
            touched.extend(edit.touched_items.iter().cloned());
        }
        Some(touched)
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
    for update in crdt.full_snapshot_updates().updates {
        // Only documents the server lacks a base for are seeded here; a document the
        // server already holds converges through the normal pull/push CRDT merge.
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
