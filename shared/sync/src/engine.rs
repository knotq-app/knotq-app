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

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use chrono::Utc;
use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, Workspace, WorkspaceId};

use crate::{
    BatchPullRequest, BatchPushRequest, CrdtDocumentUpdate, DocumentPullStateVector,
    DocumentStateVector, LocalSyncState, NotificationScheduleSnapshot, PendingCrdtEdit,
    PulledCrdtDocument, PushDocumentUpdates, StoredCrdtUpdate, WorkspaceCrdtChangeSet,
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
    /// True when the document is intentionally deferred by lazy loading (for
    /// example an off-window historical Daily Queue page), rather than failing
    /// to decode or materialize.
    pub deferred: bool,
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
    /// Documents repaired from a plain workspace/CRDT persistence mismatch
    /// before the first remote pull. These must be persisted by the platform
    /// even when the remote response is empty.
    pub locally_repaired_documents: Vec<DocumentId>,
    /// Number of pull responses consumed by this call, including the final
    /// caught-up response. Useful for distinguishing one slow request from a
    /// server page sequence in platform diagnostics.
    pub pull_requests: usize,
    /// Number and raw base64-decoded size of merged document states returned by
    /// the server during this call. These counters are diagnostics only; they
    /// do not affect convergence or cursor advancement.
    pub remote_documents_received: usize,
    /// Number of returned document states marked by the server as state-vector
    /// deltas. Full states remain a safe compatibility fallback.
    pub remote_delta_documents: usize,
    pub remote_state_bytes: usize,
    pub remote_latest: HashMap<DocumentId, u64>,
    /// CRDT document ids whose merged state changed during this pull. Drivers
    /// use this to persist only affected scheme files/state instead of doing a
    /// whole-workspace rewrite after every batched pull.
    pub changed_documents: HashSet<DocumentId>,
    /// Documents that arrived in the pull response but could not be applied
    /// locally. Their cursors were advanced anyway — see [`SkippedDocument`].
    pub skipped: Vec<SkippedDocument>,
}

/// A document whose pending edits the server accepted, with the local sequence the
/// push covered, so the caller can clear those pending edits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PushedDocument {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    pub through_local_sequence: u64,
    /// Exact local operation identities included in this acknowledgement.
    /// Landing uses these instead of only the sequence watermark so an edit
    /// created during a previous landing cannot be mistaken for an in-flight
    /// edit merely because it received the snapshot's boundary sequence.
    pub sent_edits: Vec<(OperationId, u64)>,
    /// Exact server head returned for this push. A caller may use this as a
    /// post-push pull cursor only after retaining the integrity proof: a
    /// concurrent push can still make the acknowledged head incomplete from
    /// this replica's point of view.
    pub server_sequence: u64,
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
    batch_pull_and_apply_with_integrity_check(
        transport,
        crdt_docs,
        local_state,
        workspace,
        replica_id,
        true,
    )
}

/// Pull and apply with an explicit integrity-proof decision.
///
/// The proof is intentionally opt-in at the caller boundary: it walks every
/// locally materialized CRDT document and is appropriate for startup recovery
/// and immediately after pushing local edits, but not for every websocket wake.
pub fn batch_pull_and_apply_with_integrity_check(
    transport: &dyn SyncTransport,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    local_state: &mut LocalSyncState,
    workspace: Workspace,
    replica_id: ReplicaId,
    run_integrity_check: bool,
) -> Result<PullOutcome> {
    batch_pull_and_apply_with_integrity_documents_inner(
        transport,
        crdt_docs,
        local_state,
        workspace,
        replica_id,
        run_integrity_check,
        None,
        None,
        false,
    )
}

/// Pull and apply with a full integrity proof, including documents currently
/// held as lazy bytes. This is reserved for interrupted-sync recovery; the
/// normal compatibility wrapper intentionally keeps deferred documents lazy.
pub fn batch_pull_and_apply_with_full_integrity_check(
    transport: &dyn SyncTransport,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    local_state: &mut LocalSyncState,
    workspace: Workspace,
    replica_id: ReplicaId,
    run_integrity_check: bool,
) -> Result<PullOutcome> {
    batch_pull_and_apply_with_integrity_documents_inner(
        transport,
        crdt_docs,
        local_state,
        workspace,
        replica_id,
        run_integrity_check,
        None,
        None,
        true,
    )
}

/// Pull with a startup integrity proof backed by vectors persisted at the last
/// durable checkpoint. This keeps deferred scheme histories lazy: the client
/// can still detect a server-side state change without decoding every cold
/// document just to reconstruct its vector.
pub fn batch_pull_and_apply_with_persisted_integrity_vectors(
    transport: &dyn SyncTransport,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    local_state: &mut LocalSyncState,
    workspace: Workspace,
    replica_id: ReplicaId,
    run_integrity_check: bool,
    persisted_vectors: Option<&HashMap<DocumentId, String>>,
) -> Result<PullOutcome> {
    batch_pull_and_apply_with_integrity_documents_inner(
        transport,
        crdt_docs,
        local_state,
        workspace,
        replica_id,
        run_integrity_check,
        None,
        persisted_vectors,
        false,
    )
}

/// Pull and apply with an optional integrity proof limited to selected
/// documents. A full proof (`documents == None`) is used for startup recovery;
/// a selected proof is used after a push so a one-character edit does not
/// decode and hash every scheme in the workspace.
pub fn batch_pull_and_apply_with_integrity_documents(
    transport: &dyn SyncTransport,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    local_state: &mut LocalSyncState,
    workspace: Workspace,
    replica_id: ReplicaId,
    run_integrity_check: bool,
    documents: Option<&HashSet<DocumentId>>,
) -> Result<PullOutcome> {
    batch_pull_and_apply_with_integrity_documents_inner(
        transport,
        crdt_docs,
        local_state,
        workspace,
        replica_id,
        run_integrity_check,
        documents,
        None,
        false,
    )
}

// This pipeline keeps the transport, durable sync state, CRDT documents, and
// integrity-proof options explicit because each has a distinct failure and
// persistence contract. A parameter object would obscure those boundaries at
// the most correctness-sensitive call site.
#[allow(clippy::too_many_arguments)]
fn batch_pull_and_apply_with_integrity_documents_inner(
    transport: &dyn SyncTransport,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    local_state: &mut LocalSyncState,
    workspace: Workspace,
    replica_id: ReplicaId,
    run_integrity_check: bool,
    documents: Option<&HashSet<DocumentId>>,
    persisted_vectors: Option<&HashMap<DocumentId, String>>,
    hydrate_all_deferred_for_integrity: bool,
) -> Result<PullOutcome> {
    let mut workspace = workspace;
    // The plain workspace and the durable CRDT state are separate persistence
    // artifacts. A crash or an older build can leave a locally-created scheme
    // in the former but not the latter. If we pull first, materializing the
    // remote workspace index treats that missing CRDT entry as authoritative
    // absence and silently drops the local scheme. Reconcile this boundary
    // before *any* remote bytes are applied; the resulting full snapshots are
    // durable pending edits, so the server gets the local document as well.
    let local_repair =
        queue_local_only_documents_before_pull(crdt_docs, local_state, &workspace, replica_id);
    let mut remote_updates_applied = 0;
    let mut pull_requests = 0;
    let mut remote_documents_received = 0;
    let mut remote_delta_documents = 0;
    let mut remote_state_bytes = 0;
    let mut authoritative_remote_latest: Option<HashMap<DocumentId, u64>> = None;
    let mut all_skipped: Vec<SkippedDocument> = Vec::new();
    let mut changed_documents: HashSet<DocumentId> = HashSet::new();
    let mut account_switch_merges: HashMap<DocumentId, crate::AccountSwitchMerge> = HashMap::new();
    // The first pull after an account switch: every cursor was reset and the
    // re-seed is still owed. Decided once, before the pages advance the
    // cursors. A later pull while the re-seed is still owed (its push failed)
    // is an ordinary one: by then a row missing locally may be one the user
    // removed on this account, which must stay removed.
    let account_switch_first_pull =
        local_state.needs_full_reseed() && local_state.document_cursors.is_empty();
    // A state-vector proof is tiny compared with a merged document, but it still
    // walks every decoded document on a caught-up pull. Never put that workspace-
    // wide work on the local-edit path: pending edits are about to be pushed, so
    // their state vectors are expected to differ from the server's until the push
    // completes. The next caught-up sync performs the proof once the local queue
    // is empty. This keeps a one-character edit to one ordinary pull request,
    // rather than turning it into a scan of hundreds of documents.
    let mut integrity_check_pending = run_integrity_check && local_state.pending.is_empty();
    // Keep a private, mutable copy so a deferred proof can refresh the vectors
    // for documents that were just merged. The durable startup cache describes
    // the pre-pull state; reusing it after a changed page would report a false
    // mismatch for the very update we just accepted.
    let mut persisted_vectors_for_request = persisted_vectors.cloned();
    let mut integrity_recheck_requested = false;
    // Hard backstop against a pull loop that cannot make progress (e.g. a
    // document that never materializes, so its cursor keeps getting reset). One
    // page per document plus the deferred integrity re-check is a handful of
    // iterations even for a workspace at the document cap; anything beyond this
    // is a bug. We `break` (not error) so the workspace and cursors advanced so
    // far are still persisted by the caller.
    const MAX_PULL_LOOP_ITERATIONS: u32 = 32;
    // Per-document budget for the re-convergence cursor reset below. A document
    // that legitimately needs a re-pull (its index entry arrived before its
    // content) converges in one. A document that comes back down and still fails
    // to materialize is a materialization bug, not a transport gap — re-pulling
    // it forever is what wedged every client. After this many resets in one
    // call we leave its cursor advanced and move on; the next full `sync_once`
    // starts the budget over.
    const MAX_CURSOR_RESETS_PER_DOCUMENT: u32 = 3;
    let mut cursor_reset_counts: HashMap<DocumentId, u32> = HashMap::new();
    // Documents that came down in a response during THIS call. If such a
    // document is still not a live local CRDT doc after we applied its full
    // state, re-pulling it cannot help — that is a materialization bug (bug B),
    // not a transport gap. Resetting its cursor anyway is what turned the pull
    // loop, and then the whole `sync_once` poll, into a livelock. We leave its
    // cursor where `mark_pulled` advanced it so the server stops re-sending it,
    // and surface it as a skipped document.
    let mut pulled_this_call: HashSet<DocumentId> = HashSet::new();
    // Integrity mismatches are authoritative after the caller has had a chance
    // to push pending local edits. The next returned full state must replace the
    // local document, not merge it — otherwise locally durable-but-unqueued CRDT
    // content survives forever even though the server is the chosen authority.
    let mut integrity_repair_documents: HashSet<DocumentId> = HashSet::new();
    if integrity_check_pending {
        if let Some(documents) = documents {
            crdt_docs.state_vectors_v1_for_documents(documents);
        } else if persisted_vectors.is_some() {
            // The vectors are already persisted; do not hydrate cold histories.
        } else if hydrate_all_deferred_for_integrity {
            crdt_docs.hydrate_all_deferred();
        }
    }
    let mut pull_loop_iteration = 0u32;
    loop {
        pull_loop_iteration += 1;
        if pull_loop_iteration > MAX_PULL_LOOP_ITERATIONS {
            eprintln!(
                "knotq sync: batch_pull_and_apply hit the {MAX_PULL_LOOP_ITERATIONS}-iteration \
                 cap; stopping the pull loop with partial progress (a document is failing to \
                 materialize — see the reset_pull_cursor lines above)"
            );
            break;
        }
        let request_integrity_state_vectors = if integrity_check_pending {
            let state_vectors = documents
                .map(|documents| crdt_docs.state_vectors_v1_for_documents(documents))
                .or_else(|| {
                    persisted_vectors_for_request.as_ref().map(|vectors| {
                        vectors
                            .iter()
                            .filter_map(|(document, encoded)| {
                                base64::engine::general_purpose::STANDARD
                                    .decode(encoded)
                                    .ok()
                                    .map(|state_vector_v1| (*document, state_vector_v1))
                            })
                            .collect()
                    })
                })
                .unwrap_or_else(|| crdt_docs.state_vectors_v1());
            state_vectors
                .into_iter()
                .map(|(document, state_vector_v1)| DocumentStateVector {
                    document,
                    state_vector_v1,
                })
                .collect()
        } else {
            Vec::new()
        };
        let integrity_scope: Option<HashSet<DocumentId>> = integrity_check_pending.then(|| {
            request_integrity_state_vectors
                .iter()
                .map(|entry| entry.document)
                .collect()
        });
        // Give a delta-capable server the state vector for every document whose
        // local CRDT history is already durable and whose cursor has advanced.
        // Use the vectors persisted at the last durable checkpoint instead of
        // re-encoding every live/deferred CRDT document on every pull. A stale
        // checkpoint vector is still a safe Yjs target: the response may contain
        // a few already-present structs, but merging them is idempotent. A missing
        // vector (new document, repaired document, or older local state) makes the
        // server return the complete merged state as before.
        let known_document_ids = crdt_docs.known_document_ids();
        let pull_state_vectors = if !local_state.integrity_state_vectors.is_empty() {
            local_state
                .integrity_state_vectors
                .iter()
                .filter_map(|(document, encoded)| {
                    let cursor = local_state.document_cursors.get(document)?;
                    if cursor.last_pulled_sequence == 0 || !known_document_ids.contains(document) {
                        return None;
                    }
                    let state_vector_v1 = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .ok()?;
                    Some(DocumentPullStateVector {
                        document: *document,
                        epoch: cursor.epoch,
                        state_vector_v1,
                    })
                })
                .collect()
        } else {
            // No checkpoint means no delta hint. In particular, do not derive a
            // hint from the current CRDT merely because a cursor exists: after an
            // account switch or an interrupted persistence boundary, that CRDT
            // can belong to a different server lineage. The first pull must be a
            // complete state; a later durable checkpoint can opt into deltas.
            Vec::new()
        };
        let request = BatchPullRequest {
            replica_id,
            cursors: local_state
                .document_cursors
                .values()
                .map(|cursor| (cursor.document, cursor.last_pulled_sequence))
                .collect(),
            client_protocol_version: crate::CLIENT_SYNC_PROTOCOL_VERSION,
            integrity_state_vectors: request_integrity_state_vectors,
            state_vectors: pull_state_vectors,
        };
        let response = transport.pull(&request)?;
        pull_requests += 1;
        remote_documents_received += response.documents.len();
        remote_delta_documents += response
            .documents
            .iter()
            .filter(|document| document.state_v1_is_delta)
            .count();
        remote_state_bytes += response
            .documents
            .iter()
            .map(|document| document.state_v1.len())
            .sum::<usize>();
        if let Some(mismatches) = &response.integrity_mismatches {
            integrity_check_pending = false;
            // A mismatch caused by a local edit is expected: the local CRDT is
            // ahead of the server until its pending update is pushed. Leave those
            // documents alone so the normal push phase sends local work before we
            // consider any remote re-pull. A mismatch with no pending local work
            // is actionable when it belongs to this proof's submitted scope,
            // including when that document is currently deferred. Hydrating a
            // reported deferred document turns that one history into a real
            // local doc, so the pull can merge it and the caller can persist/
            // reindex the repaired content.
            let actionable: Vec<DocumentId> = mismatches
                .iter()
                .copied()
                // Older production servers interpreted a scoped proof as if
                // omitted documents were mismatches. Those ids are outside
                // this proof and must never trigger a cursor reset/re-download
                // on the client. New servers return only in-scope ids, so this
                // is both a compatibility guard and the correct scoped-proof
                // semantics.
                .filter(|document| {
                    integrity_scope
                        .as_ref()
                        .is_some_and(|scope| scope.contains(document))
                })
                .filter(|document| !local_state.has_pending_for_document(*document))
                .filter(|document| !crdt_docs.owns_deferred_document(*document))
                .collect();
            if !actionable.is_empty() {
                for document in actionable {
                    crdt_docs.hydrate_deferred_document(document);
                    integrity_repair_documents.insert(document);
                    local_state.reset_pull_cursor(document);
                }
                // The backend has identified the exact bad documents. Fetch
                // only those full states; no workspace-wide reset is needed.
                continue;
            }
        }
        if let Some(known_documents) = &response.known_documents {
            authoritative_remote_latest = Some(known_documents.clone());
            // A server head lower than our saved cursor (or a saved cursor for a
            // document the server no longer has) is a precise, cheap signal of
            // stale local state. Repair only the offending cursor and immediately
            // repeat the pull; do not turn a normal manual sync into a full
            // workspace download.
            if local_state.reconcile_server_heads(known_documents) {
                continue;
            }
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
                && (integrity_repair_documents.contains(&doc.document)
                    || local_state
                        .document_cursors
                        .get(&doc.document)
                        .is_some_and(|cursor| cursor.epoch != doc.epoch))
        };
        let (adoptions, merges): (Vec<&PulledCrdtDocument>, Vec<&PulledCrdtDocument>) = response
            .documents
            .iter()
            .partition(|doc| needs_adoption(doc));
        // After an account switch the local documents still carry the account
        // being left, under document ids the destination uses too, with structs
        // the destination holds live and this device holds removed (a Daily
        // page's starter rows, a row it rolled forward there, anything it
        // carried across before). An ordinary merge keeps this device's
        // removals and the full-snapshot re-seed then pushes them onto the
        // destination's identical rows, deleting them for every device there.
        // So the switch's first pull — every cursor reset, every response a
        // full state — merges each document so that nothing the account being
        // left removed reaches what the destination still has
        // (`apply_remote_updates_for_account_switch`). A delta response cannot
        // be handled that way and takes the ordinary merge.
        let account_switch_documents: HashSet<DocumentId> = if account_switch_first_pull {
            merges
                .iter()
                .filter(|doc| !doc.state_v1_is_delta)
                .map(|doc| doc.document)
                .collect()
        } else {
            HashSet::new()
        };

        // A complete snapshot for an off-window deferred scheme can be stored
        // as bytes without hydrating that historical Yjs document. This is the
        // common cold-mobile catch-up case: the document must be retained and
        // its cursor advanced, but it cannot affect the currently materialized
        // workspace. Keep visible schemes, deltas, and documents with pending
        // local work on the normal merge path.
        let mut updates = Vec::with_capacity(merges.len());
        for doc in merges {
            let deferred_scheme = (doc.kind == SyncDocumentKind::Scheme
                && !doc.state_v1_is_delta
                && !local_state.has_pending_for_document(doc.document))
            .then(|| {
                workspace
                    .scheme_sync
                    .iter()
                    .find_map(|(scheme_id, meta)| (meta.id == doc.document).then_some(*scheme_id))
            })
            .flatten()
            .filter(|scheme_id| {
                // Only a zero-cursor bootstrap may replace deferred bytes
                // without a Yjs merge. An established document can have a
                // locally durable/index state that must be reconciled through
                // the normal path, even when the server response is a full
                // merged snapshot.
                let document = workspace.scheme_sync[scheme_id].id;
                local_state
                    .document_cursors
                    .get(&document)
                    .is_none_or(|cursor| cursor.last_pulled_sequence == 0)
            })
            .filter(|scheme_id| !workspace.schemes.contains_key(scheme_id));
            if let Some(_scheme_id) = deferred_scheme {
                if crdt_docs.replace_deferred_full_state(doc.document, &doc.state_v1) {
                    remote_updates_applied += 1;
                    changed_documents.insert(doc.document);
                    local_state.mark_deferred_materialization(doc.document);
                }
                continue;
            }
            updates.push(pulled_document_as_update(workspace_id, doc));
        }
        // `apply_remote_updates` applies workspace-kind updates (and re-materializes)
        // before scheme-kind ones, so a scheme created on another device — whose
        // workspace-index entry and scheme document arrive in the same response — is
        // routed correctly even though this replica had never seen it.
        let outcome = crdt_docs.apply_remote_updates_for_account_switch(
            &workspace,
            &updates,
            &account_switch_documents,
        );
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

        changed_documents.extend(outcome.changed_documents.iter().copied());
        account_switch_merges.extend(
            outcome
                .account_switch_merges
                .iter()
                .map(|(k, v)| (*k, v.clone())),
        );

        // The switch queued this device's workspace index for push before the
        // pull (`reidentify_workspace_document`): the index as it stood on the
        // account being left, delete set and all. That index was just rebuilt
        // remote-first, so every queued index edit — that snapshot, and any
        // older index delta whose structs the rebuilt document already carries
        // — now carries the rebuilt state instead, which holds no tombstone
        // for an entry the destination still has. Rewritten in place, under
        // the same operation ids and sequences: the store that handed these
        // edits to the run clears them by exact id once they are pushed, and a
        // replacement queued under a fresh id would leave the originals in the
        // store to be handed to every later run, pushed, and never cleared
        // (chaos seed 141 wedged that way).
        if account_switch_documents.contains(&workspace.sync.id) {
            let rebuilt = crdt_docs
                .full_snapshot_updates_for_documents(&HashSet::from([workspace.sync.id]))
                .updates
                .into_iter()
                .find(|update| update.document == workspace.sync.id);
            if let Some(rebuilt) = rebuilt {
                for edit in local_state
                    .pending
                    .iter_mut()
                    .filter(|edit| edit.kind == SyncDocumentKind::PersonalWorkspace)
                {
                    edit.document = rebuilt.document;
                    edit.update_v1 = rebuilt.update_v1.clone();
                    edit.touched_items = rebuilt.touched_items.clone();
                }
            }
        }

        // If the server had to return a changed page before it could evaluate
        // the proof, refresh those entries in the request-local cache. The
        // second request must prove the post-merge state, not the pre-pull
        // vectors loaded from durable storage.
        if integrity_check_pending && response.integrity_check_deferred {
            if let Some(cached_vectors) = persisted_vectors_for_request.as_mut() {
                let changed_page_documents: HashSet<DocumentId> = response
                    .documents
                    .iter()
                    .map(|document| document.document)
                    .collect();
                for (document, state_vector_v1) in
                    crdt_docs.state_vectors_v1_for_documents(&changed_page_documents)
                {
                    cached_vectors.insert(
                        document,
                        base64::engine::general_purpose::STANDARD.encode(state_vector_v1),
                    );
                }
            }
        }

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
                    integrity_repair_documents.remove(&doc.document);
                    workspace = adopted_workspace;
                    remote_updates_applied += 1;
                    changed_documents.insert(doc.document);
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
                        deferred: false,
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
        // An unknown scheme document can be a transient ordering issue: the
        // content page may arrive before the workspace-index page that binds
        // it.  Keep that distinction for the re-convergence pass below.  A
        // generic materialization failure should not be retried indefinitely,
        // but an unknown document must be fetched again once its index entry is
        // present or the device can remain permanently empty at a matching
        // cursor.
        let unknown_scheme_documents: HashSet<DocumentId> = errored_document_ids
            .values()
            .filter(|error| error.unknown_scheme_document)
            .map(|error| error.document)
            .collect();

        for doc in &response.documents {
            pulled_this_call.insert(doc.document);
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
                    deferred: false,
                    reason: err.message.clone(),
                });
            }
        }

        // Re-convergence: after applying workspace updates, any scheme that is
        // now in the workspace index but whose local CRDT doc is missing (or was
        // in this pull's skipped set) needs its pull cursor reset so the next
        // poll fetches its full merged state from sequence zero. This converges
        // an orphan-then-index-added sequence.
        //
        // The reset is budgeted PER DOCUMENT (`MAX_CURSOR_RESETS_PER_DOCUMENT`).
        // Without a budget this loop is unbounded: a document that comes back
        // down every page and still fails to materialize gets its cursor reset,
        // is re-pulled, still fails, is reset again — forever, re-cloning and
        // re-applying the whole workspace each pass. That is the "stuck on
        // Resyncing" wedge. After the budget is spent we leave the cursor where
        // `mark_pulled` advanced it and move on; a later `sync_once` (or a fix
        // to whatever is dropping the document) retries from a clean budget.
        let skipped_document_ids: std::collections::HashSet<DocumentId> =
            all_skipped.iter().map(|skipped| skipped.document).collect();
        let local_crdt_doc_ids = crdt_docs.known_document_ids();
        let mut materialization_gaps: Vec<(DocumentId, SyncDocumentKind)> = Vec::new();
        for (scheme_id, meta) in &workspace.scheme_sync {
            if meta.kind != SyncDocumentKind::Scheme {
                continue;
            }
            let missing_locally = !local_crdt_doc_ids.contains(&meta.id);
            let was_skipped = skipped_document_ids.contains(&meta.id);
            if !(missing_locally || was_skipped) {
                continue;
            }
            let ever_pulled = local_state
                .document_cursors
                .get(&meta.id)
                .is_some_and(|cursor| cursor.last_pulled_sequence > 0);
            let needs_index_ordering_retry =
                was_skipped && unknown_scheme_documents.contains(&meta.id) && missing_locally;
            if (pulled_this_call.contains(&meta.id) || ever_pulled) && !needs_index_ordering_retry {
                // Its full state has already come down (this call, or an earlier
                // sync — its cursor is advanced) and it is still not a live local
                // document. That is a workspace-index inconsistency (the index
                // binds the scheme in `scheme_sync` but never materializes a node
                // for it), not a transport gap. Re-pulling cannot fix it and
                // resetting the cursor to 0 makes the server resend it every
                // pull — the "stuck on Resyncing" livelock. Leave the cursor
                // advanced so the server stops resending it, and record the gap.
                materialization_gaps.push((meta.id, meta.kind));
                continue;
            }
            let count = cursor_reset_counts.entry(meta.id).or_insert(0);
            if *count >= MAX_CURSOR_RESETS_PER_DOCUMENT {
                // Giving up on a document silently is how a page goes missing
                // with nothing to show for it: the caller sees a clean pull, no
                // skipped entry, and a workspace with one scheme fewer. Record
                // it as a gap so it is reported like any other.
                materialization_gaps.push((meta.id, meta.kind));
                continue;
            }
            *count += 1;
            local_state.reset_pull_cursor(meta.id);
            let _ = scheme_id; // used via meta
        }
        for (document, kind) in materialization_gaps.drain(..) {
            if skipped_document_ids.contains(&document) {
                continue;
            }
            all_skipped.push(SkippedDocument {
                document,
                kind,
                unknown_scheme_document: false,
                deferred: true,
                reason: "pulled but did not materialize into a local CRDT document".to_string(),
            });
        }

        if !response.has_more {
            // A new server defers an integrity proof while it returns changed
            // documents, because the vectors in this request necessarily
            // describe the pre-merge local state. Re-run the check ONCE after
            // applying the page — clearing `integrity_check_pending` here makes
            // it a one-shot, so a document that keeps coming back cannot keep
            // this deferred re-check alive and spin the loop. Older servers omit
            // the flag and are unaffected.
            if integrity_check_pending
                && response.integrity_check_deferred
                && !integrity_recheck_requested
            {
                integrity_recheck_requested = true;
                continue;
            }
            // A server that keeps returning changed pages has not given us
            // a caught-up proof opportunity yet. Spend the one re-check
            // budget and finish the pull; the next sync can try again.
            break;
        }
    }
    // Every page of the switch's pull is in: keep the rows the merge would
    // otherwise cost (see `revive_after_account_switch`).
    if !account_switch_merges.is_empty() {
        let (revived_workspace, written) = crdt_docs
            .revive_after_account_switch(&workspace, &account_switch_merges)
            .context("account-switch revival")?;
        workspace = revived_workspace;
        changed_documents.extend(written);
    }
    // A cursor proves that this replica received a server document version; it
    // does *not* prove that the separately-persisted, UI-facing `Workspace`
    // was successfully materialized from that CRDT state. In particular, an
    // interrupted save can leave the plain workspace stale while the CRDT state
    // and cursor are both current. The server correctly returns an empty
    // response in that case, which used to let both devices report "synced"
    // while rendering different content.
    //
    // Rebuild once after every pull, including an empty one. This is local-only
    // (no additional request, wake-up, or document download) and changes the
    // workspace only when the CRDT's authoritative materialization differs.
    // Count a repair as remote work so platform drivers durably save it before
    // they persist the already-advanced cursors.
    //
    // This uses the ordinary (not the exhaustive-diagnostic) materialization:
    // it repairs the schemes this replica has decoded — ordinary schemes and
    // the visible daily window — so a caught-up pull's cost tracks the
    // visible/touched set rather than the total historical daily count. An
    // off-window daily that failed to parse is repaired the moment the UI
    // touches that date (which decodes its intact CRDT bytes), not here.
    //
    // A scheme whose content document has never synced (a daily just created
    // locally, a scheme mid-bootstrap) may have an empty local CRDT document
    // while its real content sits only in the plain workspace, waiting to be
    // pushed. The repair must keep that content, not treat the empty document
    // as authoritative — so only an empty CRDT document for an already-synced
    // scheme is trusted here.
    // On the ordinary mobile wake path, an empty response means there was no
    // new remote CRDT state to reconcile and the previous successful cycle has
    // already persisted the workspace/CRDT pair. Rebuilding the visible
    // workspace here would turn every idle websocket nudge into a local scan.
    // Keep the verification for recovery/integrity pulls and for any response
    // that actually carried documents; those are the boundaries where stale
    // materialization can be introduced. The legacy wrapper still passes
    // `run_integrity_check = true`, so desktop and diagnostic callers retain
    // their existing no-op repair behavior.
    let should_verify_materialization =
        run_integrity_check || remote_documents_received > 0 || !changed_documents.is_empty();
    let synced_scheme_documents: HashSet<DocumentId> = local_state
        .document_cursors
        .values()
        .filter(|cursor| cursor.last_pulled_sequence > 0 || cursor.last_pushed_sequence > 0)
        .map(|cursor| cursor.document)
        .collect();
    let scheme_document_is_synced = |scheme_id: &knotq_model::SchemeId| {
        workspace
            .scheme_sync
            .get(scheme_id)
            .is_some_and(|meta| synced_scheme_documents.contains(&meta.id))
    };
    if should_verify_materialization {
        let materialized = crdt_docs
            .materialized_workspace_repair(&workspace, &scheme_document_is_synced)
            .context("verify workspace materialization after sync pull")?;
        if materialized != workspace {
            workspace = materialized;
            remote_updates_applied += 1;
        }
        // Do not tombstone the losing side of a duplicate item id here. A
        // cross-document move is represented by independent CRDT documents;
        // while a stale source delete and a destination insert are in flight,
        // another replica can legitimately hold the same id in both. The
        // materializer's deterministic dedupe gives every replica one visible
        // owner. Destructively rewriting the losing document is unsafe: a
        // replica that has not seen the move can then tombstone the other copy,
        // and the item disappears from every document. Keeping both histories
        // is lossless and convergent; later edits or a deliberate delete provide
        // the only safe evidence for a tombstone.
    }

    let locally_repaired_documents = if let Some(repair) = local_repair {
        // A remote scheme document may contain tombstones or a partial state
        // that would still win over the pre-pull seed. Re-express only the
        // specific scheme content documents that were detected as locally
        // ahead. The workspace-index snapshot was already queued before the
        // pull; replaying the entire pre-pull workspace here would resurrect
        // unrelated remote folder/scheme changes that arrived during the pull.
        let repaired_schemes: Vec<_> = repair.changeset.schemes.iter().copied().collect();
        // Re-express only what was ahead of the CRDT before the pull. Written
        // as the pre-pull plain copy stood, a scheme would lose every line the
        // pull just delivered (another device's additions): a scheme write
        // makes the document match it exactly.
        let mut repair_workspace = repair.workspace.clone();
        if let Ok(pulled) = crdt_docs.materialized_workspace_repair(&workspace, &|_| false) {
            for scheme_id in &repaired_schemes {
                let (Some(ahead), Some(remote), Some(local)) = (
                    repair.local_ahead_items.get(scheme_id),
                    pulled.schemes.get(scheme_id),
                    repair_workspace.schemes.get_mut(scheme_id),
                ) else {
                    continue;
                };
                // `remote` is the materialized view, so deterministic
                // cross-scheme dedupe may have hidden a live copy that is
                // still present in this scheme's raw CRDT document. Keep
                // those raw-only entries in the repair input and mark them
                // touched: rewriting from the deduped view would otherwise
                // tombstone the losing copy, and a later source move/delete
                // could make the item disappear everywhere.
                let raw_items = crdt_docs
                    .raw_scheme_items(*scheme_id)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let raw_ids: HashSet<String> =
                    raw_items.iter().map(|item| item.id.to_string()).collect();
                let mut local_items =
                    crate::crdt::merge_raw_only_items(local, raw_items, None).items;
                let remote_ids: HashSet<String> = remote
                    .items
                    .iter()
                    .map(|item| item.id.to_string())
                    .collect();
                let mut touched = ahead.clone();
                touched.extend(raw_ids.into_iter().filter(|id| !remote_ids.contains(id)));
                let merged = crate::crdt::merge_items_for_adoption(
                    &local_items,
                    remote.items.clone(),
                    &touched,
                );
                local_items = merged;
                local.items = local_items;
            }
        }
        let outcome = crdt_docs.sync_scheme_documents(&repair_workspace, &repaired_schemes);
        for error in &outcome.errors {
            eprintln!("knotq sync: post-pull local CRDT repair skipped: {error}");
        }
        queue_crdt_updates(local_state, &repair.workspace, replica_id, outcome.updates);
        let repaired_workspace = crdt_docs
            .materialized_workspace_repair(&workspace, &|_| false)
            .context("materialize post-pull local CRDT repair")?;
        if repaired_workspace != workspace {
            workspace = repaired_workspace;
        }
        repair.documents
    } else {
        Vec::new()
    };
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
        locally_repaired_documents,
        pull_requests,
        remote_documents_received,
        remote_delta_documents,
        remote_state_bytes,
        remote_latest,
        changed_documents,
        skipped: all_skipped,
    })
}

/// Repair the gap between a plain workspace snapshot and its persisted CRDT
/// documents before pulling remote state. This is deliberately in the shared
/// engine so desktop and mobile get the same data-loss guard.
struct LocalPrePullRepair {
    workspace: Workspace,
    changeset: WorkspaceCrdtChangeSet,
    documents: Vec<DocumentId>,
    /// For each scheme repaired because its plain file was ahead of its CRDT,
    /// the items that were ahead: added, changed or deleted locally. Only
    /// these are re-expressed after the pull.
    local_ahead_items: HashMap<knotq_model::SchemeId, HashSet<String>>,
}

/// Say what the pre-pull repair decided. Behind an env var: it runs on every
/// sync and the common answer is "nothing to repair".
fn trace_pre_pull_repair(what: &str) {
    if std::env::var("KNOTQ_TRACE_PRE_PULL_REPAIR").is_ok() {
        eprintln!("sync: pre-pull local-only repair {what}");
    }
}

fn queue_local_only_documents_before_pull(
    crdt_docs: &mut WorkspaceCrdtDocuments,
    local_state: &mut LocalSyncState,
    workspace: &Workspace,
    replica_id: ReplicaId,
) -> Option<LocalPrePullRepair> {
    // Account switches intentionally defer to the post-pull re-seed path: the
    // old CRDT belongs to the source account and must not be pushed into the
    // destination account before its workspace index has been adopted.
    if local_state.needs_full_reseed() {
        trace_pre_pull_repair("skipped: account switch pending a full re-seed");
        return None;
    }
    // An unseeded workspace document has no history to merge with, so it must
    // adopt the server's workspace index before anything local is written into
    // it. Writing the plain workspace first mints an independent index that wins
    // over the server's when the two merge, deleting the account's existing
    // schemes, folders and days for every device. That is not only the empty
    // bootstrap: a real first launch seeds the starter workspace, so a new
    // install signing into an existing account has a non-empty plain workspace
    // and an unseeded CRDT (`fresh_install_join.rs`). The repair below is for a
    // seeded CRDT that fell behind the plain files.
    if !crdt_docs.workspace_is_seeded() {
        trace_pre_pull_repair("skipped: workspace document not seeded yet");
        return None;
    }
    // The repair is for a CRDT that already synced with this server and then
    // fell behind the plain files. A device that has never synced with this
    // server — no pull or push cursor at all — has nothing for its plain files to
    // be ahead of: its offline history reaches the account through the
    // re-identified workspace snapshot and the post-pull bootstrap, like any
    // first sign-in. Running the repair first instead writes the pre-sign-in
    // workspace index — local root, local identity — over the account's, and the
    // account loses everything (`offline_device_join.rs`). A first sign-in does
    // not trip `needs_full_reseed`: there is no previous account to reset from.
    // The repair is for a CRDT that already synced with this server and then
    // fell behind the plain files. A device that has never synced with this
    // server — no pull or push cursor at all — has nothing for its plain files to
    // be ahead of: its offline history reaches the account through the
    // re-identified workspace snapshot and the post-pull bootstrap, like any
    // first sign-in. Running the repair first instead writes the pre-sign-in
    // workspace index — local root, local identity — over the account's, and the
    // account loses everything (`offline_device_join.rs`). A first sign-in does
    // not trip `needs_full_reseed`: there is no previous account to reset from.
    //
    // Letting the CONTENT half run here while suppressing only the index was
    // tried and took the release-depth gate from 5 failing seeds to 17: on a
    // device whose plain files still hold starter content the account has since
    // deleted, re-asserting that content before the pull resurrects it. Chaos
    // seed 253 — a line lost because this repair is skipped — needs a fix that
    // can tell unpublished user content from unpublished starter content, which
    // this cannot.
    // A device with no cursors has never synced with this server. Its workspace
    // INDEX must not be written before the account's is pulled — that index is
    // its own, local root and all, and writing it first costs the account
    // everything (`offline_device_join.rs`) — and most of its plain content is
    // not really its own either: a fresh install's starter lines are the same
    // lines the account may have deleted long ago, and re-asserting them here
    // resurrects them.
    //
    // Skipping the whole repair for that reason took scheme content with it,
    // and a line this device actually authored is then dropped by the pull's
    // materialization with nothing able to bring it back (chaos seed 253:
    // device 0 inserts a line at step 19, every sync until step 148 fails, and
    // that first successful sync loses it).
    //
    // Both can be true at once. A starter line's id is FIXED — derived, so
    // byte-identical on every install — while a line someone typed gets a
    // random v4 id that exists nowhere else by construction. So on a first
    // sync, repair only the ids that cannot be starter content, and leave the
    // index alone entirely.
    let first_sync_with_this_server = local_state.document_cursors.is_empty();
    if first_sync_with_this_server {
        trace_pre_pull_repair("first sync: index repair suppressed, authored lines only");
    }
    let known_documents = crdt_docs.known_document_ids();
    let mut missing_schemes = workspace
        .scheme_sync
        .iter()
        .filter_map(|(scheme_id, metadata)| {
            (metadata.kind == SyncDocumentKind::Scheme
                && workspace.schemes.contains_key(scheme_id)
                && !known_documents.contains(&metadata.id))
            .then_some((*scheme_id, metadata.id))
        })
        .collect::<Vec<_>>();
    if first_sync_with_this_server {
        missing_schemes.clear();
    }
    let workspace_index_mismatch = !first_sync_with_this_server
        && match crdt_docs.workspace_folder_records_match(workspace) {
            Ok(matches) => !matches,
            Err(error) => {
                // An unreadable comparison is not permission to discard the
                // plain workspace. Queue a reconciliation; the normal CRDT
                // validation will reject only the repair itself if the bytes
                // are unusable.
                eprintln!(
                    "knotq sync: could not compare plain and CRDT workspace indexes: {error:#}"
                );
                true
            }
        };
    missing_schemes.sort_by_key(|(scheme_id, _)| *scheme_id);

    // The workspace index can be perfectly current while a plain scheme file
    // is newer than its separately persisted CRDT document. Detect that case
    // before the early return as well; otherwise a remote change to an
    // unrelated document gives the stale CRDT a chance to overwrite the newer
    // plain scheme content.
    let mut local_ahead_items: HashMap<knotq_model::SchemeId, HashSet<String>> = HashMap::new();
    // Per scheme, the raw CRDT items as they stand now, for schemes whose plain
    // copy is missing some of them (see below). This deliberately does not use
    // `materialized_workspace_repair`: that view has already hidden duplicate
    // ids in their deterministic winning scheme, while the losing document's
    // live copy still must not be rewritten as a deletion.
    let mut crdt_only_items: HashMap<knotq_model::SchemeId, Vec<knotq_model::Item>> =
        HashMap::new();
    for (scheme_id, local) in &workspace.schemes {
        let Some(crdt_items) = crdt_docs.raw_scheme_items(*scheme_id).ok().flatten() else {
            continue;
        };
        if local.items == crdt_items
            // An empty plain scheme is the known stale-file shape: a failed
            // materialization/save can clear the UI snapshot while the
            // durable CRDT still has content. Never turn that into a
            // deletion; the normal materialization pass restores it.
            || (local.items.is_empty() && !crdt_items.is_empty())
        {
            continue;
        }
        let crdt_by_id: HashMap<String, &knotq_model::Item> = crdt_items
            .iter()
            .map(|item| (item.id.to_string(), item))
            .collect();
        let local_ids: HashSet<String> =
            local.items.iter().map(|item| item.id.to_string()).collect();
        let ahead: HashSet<String> = local
            .items
            .iter()
            .filter(|item| {
                crdt_by_id
                    .get(&item.id.to_string())
                    .is_none_or(|crdt_item| **crdt_item != **item)
            })
            .map(|item| item.id.to_string())
            .collect();
        // Lines the CRDT has and the plain copy does not are NOT treated as
        // local deletions. "Plain lacks it" is ambiguous — the user deleted
        // it, these files are simply behind the durable CRDT, or materialization
        // kept the same id in another scheme. Reading it as a deletion made the
        // repair tombstone lines nobody deleted, and an account switch then
        // carried those tombstones into the destination account, where every
        // device lost them (production fuzz seed 6: five lines across four
        // schemes). A real deletion reaches the CRDT through the edit's own
        // flush, not through this repair.
        let missing_from_plain: Vec<_> = crdt_items
            .iter()
            .filter(|item| !local_ids.contains(&item.id.to_string()))
            .cloned()
            .collect();
        if !missing_from_plain.is_empty() && !first_sync_with_this_server {
            // Preserve the complete raw scheme snapshot while reconciling. The
            // materialized workspace may intentionally omit a duplicate copy,
            // but replace_scheme would otherwise turn that omission into a
            // destructive tombstone.
            crdt_only_items.insert(*scheme_id, crdt_items);
        }
        // On a first sync, only lines this device can prove it authored: a
        // random (v4) id. A derived id is generated — starter content, a
        // carryover's archived row — and may be something the account deleted
        // before this device ever reached it.
        let ahead: HashSet<String> = if first_sync_with_this_server {
            ahead
                .into_iter()
                .filter(|item| {
                    item.parse::<knotq_model::ItemId>()
                        .is_ok_and(|id| id.0.get_version_num() == 4)
                })
                .collect()
        } else {
            ahead
        };
        let carries_crdt_only = !missing_from_plain.is_empty() && !first_sync_with_this_server;
        if !ahead.is_empty() || carries_crdt_only {
            local_ahead_items.insert(*scheme_id, ahead);
        }
    }
    let content_mismatch_schemes: HashSet<knotq_model::SchemeId> =
        local_ahead_items.keys().copied().collect();
    if missing_schemes.is_empty()
        && !workspace_index_mismatch
        && content_mismatch_schemes.is_empty()
    {
        trace_pre_pull_repair("nothing to repair");
        return None;
    }
    trace_pre_pull_repair(&format!(
        "repairing {} missing scheme doc(s), index_mismatch={workspace_index_mismatch}, {} scheme(s) with local-ahead content",
        missing_schemes.len(),
        content_mismatch_schemes.len()
    ));
    // Only a repair that rewrites the workspace index replaces the index edits
    // queued before it (with a full snapshot, below). A repair of scheme
    // content alone leaves them queued: dropping them there discarded this
    // device's folder and scheme edits outright. Likewise a missing scheme
    // document's queued deltas are replaced by its full snapshot, since the
    // server would reject a bare delta for a document it has no base for.
    let rewrites_index = workspace_index_mismatch || !missing_schemes.is_empty();
    local_state.pending.retain(|edit| {
        (!rewrites_index || edit.kind != SyncDocumentKind::PersonalWorkspace)
            && !missing_schemes
                .iter()
                .any(|(_, document)| *document == edit.document)
    });

    let mut changeset = WorkspaceCrdtChangeSet {
        workspace: rewrites_index,
        ..WorkspaceCrdtChangeSet::default()
    };
    changeset
        .schemes
        .extend(missing_schemes.iter().map(|(scheme_id, _)| *scheme_id));
    changeset.schemes.extend(content_mismatch_schemes);
    // Only an index repair may write the index; a content-only repair writes
    // just the mismatched schemes (see `sync_scheme_documents`).
    // Write a MERGE of the plain copy and the document, never the bare plain
    // copy: `replace_scheme` makes the document match exactly, so passing the
    // plain scheme tombstones every line only the document holds. The plain
    // copy wins for the lines it actually changed (`local_ahead_items`); the
    // rest keep the document's version, and plain-only lines are added.
    let mut repair_source = Cow::Borrowed(workspace);
    if !crdt_only_items.is_empty() {
        let mut merged_workspace = workspace.clone();
        for (scheme_id, crdt_items) in &crdt_only_items {
            let (Some(ahead), Some(local)) = (
                local_ahead_items.get(scheme_id),
                merged_workspace.schemes.get_mut(scheme_id),
            ) else {
                continue;
            };
            local.items =
                crate::crdt::merge_items_for_adoption(&local.items, crdt_items.clone(), ahead);
        }
        repair_source = Cow::Owned(merged_workspace);
    }
    let repair_source = repair_source.as_ref();
    let outcome = if changeset.workspace {
        crdt_docs.sync_changes(repair_source, &changeset)
    } else {
        let schemes: Vec<_> = changeset.schemes.iter().copied().collect();
        crdt_docs.sync_scheme_documents(repair_source, &schemes)
    };
    for error in &outcome.errors {
        eprintln!("knotq sync: pre-pull local CRDT repair skipped: {error}");
    }

    // The documents whose queued edits were dropped above go out as FULL
    // snapshots. `sync_changes` only diffs: it writes the plain workspace into
    // documents that already hold those queued edits, so its delta for them is
    // nearly empty, and queueing it in their place silently discarded them —
    // the server accepted a delete-only update whose dependencies it lacked and
    // dropped it, while this device cleared the real edits as pushed.
    let mut full_documents: HashSet<DocumentId> = missing_schemes
        .iter()
        .map(|(_, document)| *document)
        .collect();
    if changeset.workspace {
        full_documents.insert(workspace.sync.id);
    }
    let mut updates = crdt_docs
        .full_snapshot_updates_for_documents(&full_documents)
        .updates;
    updates.sort_by_key(|update| update.kind != SyncDocumentKind::PersonalWorkspace);
    updates.extend(
        outcome
            .updates
            .into_iter()
            .filter(|update| !full_documents.contains(&update.document)),
    );
    let documents = updates
        .iter()
        .map(|update| update.document)
        .collect::<Vec<_>>();
    queue_crdt_updates(local_state, workspace, replica_id, updates);
    Some(LocalPrePullRepair {
        workspace: repair_source.clone(),
        changeset,
        documents,
        local_ahead_items,
    })
}

fn queue_crdt_updates(
    local_state: &mut LocalSyncState,
    workspace: &Workspace,
    replica_id: ReplicaId,
    updates: Vec<CrdtDocumentUpdate>,
) {
    if updates.is_empty() {
        return;
    }
    let operation_id = OperationId::new();
    let next_sequence = local_state
        .pending
        .iter()
        .map(|edit| edit.local_sequence)
        .max()
        .unwrap_or(0)
        + 1;
    for (next_sequence, update) in (next_sequence..).zip(updates) {
        local_state.push_pending(PendingCrdtEdit {
            operation_id,
            workspace_id: workspace.id,
            replica_id,
            local_sequence: next_sequence,
            created_at: Utc::now(),
            document: update.document,
            kind: update.kind,
            update_v1: update.update_v1,
            touched_items: update.touched_items,
        });
    }
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
// The push path intentionally receives the independent durable state, transport,
// CRDT documents, workspace view, and notification snapshot separately: callers
// persist/clear them at different points when a batch partially succeeds.
#[allow(clippy::too_many_arguments)]
pub fn batch_push_pending(
    transport: &dyn SyncTransport,
    local_state: &mut LocalSyncState,
    replica_id: ReplicaId,
    notification_schedule: &NotificationScheduleSnapshot,
    background_refresh_required: bool,
    pushed: &mut Vec<PushedDocument>,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    workspace: &Workspace,
) -> Result<()> {
    // Track which documents we've already reseeded this call; a second rejection
    // after reseed means something is deeply wrong — propagate that error.
    let mut reseeded: HashSet<DocumentId> = HashSet::new();
    loop {
        let Some((request, acks)) = build_push_request(
            local_state,
            replica_id,
            notification_schedule,
            background_refresh_required,
        ) else {
            // Keep the account-switch guard armed until the complete push has
            // drained. Clearing it when snapshots are merely queued lets a
            // failed push run the pre-pull local repair on the next attempt,
            // allowing source-account tombstones to overwrite destination data.
            local_state.clear_full_reseed();
            return Ok(());
        };
        let push_result = transport.push(&request);
        match push_result {
            Ok(response) => {
                let response_by_document: HashMap<DocumentId, (usize, u64)> = response
                    .documents
                    .iter()
                    .map(|doc| (doc.document, (doc.accepted, doc.seq)))
                    .collect();
                for (sent, ack) in request.documents.iter().zip(acks.iter()) {
                    let Some((accepted, server_sequence)) =
                        response_by_document.get(&ack.document).copied()
                    else {
                        return Err(anyhow!(
                            "sync backend omitted push acknowledgement for {}",
                            ack.document
                        ));
                    };
                    if accepted != sent.updates.len() {
                        return Err(anyhow!(
                            "sync backend accepted {accepted}/{} updates for {}",
                            sent.updates.len(),
                            ack.document
                        ));
                    }
                    // Use exact edit-ID clearing so duplicate-sequence edits from a
                    // legacy restart are not silently dropped.
                    local_state.mark_pushed_edits(ack.document, &ack.sent_edits);
                    pushed.push(PushedDocument {
                        document: ack.document,
                        kind: sent.kind,
                        through_local_sequence: ack.through_local_sequence,
                        sent_edits: ack.sent_edits.clone(),
                        server_sequence,
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
                    let snapshot_documents = HashSet::from([ack.document]);
                    let snapshot_updates =
                        crdt_docs.full_snapshot_updates_for_documents(&snapshot_documents);
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

const SQUASH_MIN_STATE_BYTES_ENV: &str = "KNOTQ_SQUASH_MIN_STATE_BYTES";
const SQUASH_MIN_RATIO_ENV: &str = "KNOTQ_SQUASH_MIN_RATIO";

fn squash_min_state_bytes() -> usize {
    std::env::var(SQUASH_MIN_STATE_BYTES_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SQUASH_MIN_STATE_BYTES)
}

fn squash_min_ratio() -> usize {
    std::env::var(SQUASH_MIN_RATIO_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SQUASH_MIN_RATIO)
}

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
///
/// Size gates keep this from ever firing on healthy documents. Returns `None`
/// when nothing qualifies — the common case.
pub fn build_squash_proposal(
    crdt_docs: &WorkspaceCrdtDocuments,
    local_state: &LocalSyncState,
) -> Option<SquashProposal> {
    let min_state_bytes = squash_min_state_bytes();
    let min_ratio = squash_min_ratio();
    for (document, state_len) in crdt_docs.squash_candidates(min_state_bytes) {
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
        if state_v1.len().saturating_mul(min_ratio) > state_len {
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
    background_refresh_required: bool,
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
            background_refresh_required,
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
    use crate::{BatchPullResponse, BatchPushResponse, PushedCrdtDocument};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

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

    struct PushAckTransport {
        server_sequence: u64,
    }

    impl SyncTransport for PushAckTransport {
        fn pull(&self, _request: &BatchPullRequest) -> Result<BatchPullResponse> {
            Ok(BatchPullResponse::default())
        }

        fn push(&self, request: &BatchPushRequest) -> Result<BatchPushResponse> {
            Ok(BatchPushResponse {
                documents: request
                    .documents
                    .iter()
                    .map(|document| PushedCrdtDocument {
                        document: document.document,
                        seq: self.server_sequence,
                        accepted: document.updates.len(),
                    })
                    .collect(),
                ..BatchPushResponse::default()
            })
        }
    }

    struct DeferredIntegrityTransport {
        requests: RefCell<Vec<BatchPullRequest>>,
        responses: RefCell<VecDeque<BatchPullResponse>>,
    }

    impl SyncTransport for DeferredIntegrityTransport {
        fn pull(&self, request: &BatchPullRequest) -> Result<BatchPullResponse> {
            self.requests.borrow_mut().push(request.clone());
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| anyhow!("test transport ran out of pull responses"))
        }

        fn push(&self, _request: &BatchPushRequest) -> Result<BatchPushResponse> {
            Ok(BatchPushResponse::default())
        }
    }

    /// Replays the same unmaterializable scheme page on every pull. This is the
    /// production failure mode behind the old "Sync now never does anything"
    /// symptom: the workspace index binds the document, but the local CRDT has
    /// no scheme node for it, so the old engine reset the cursor and asked for
    /// the same page forever. A desktop manual-sync signal queued behind that
    /// run could not be consumed until this returned.
    struct RepeatingMaterializationGapTransport {
        pull_calls: Cell<usize>,
        document: DocumentId,
        state_v1: Vec<u8>,
    }

    impl SyncTransport for RepeatingMaterializationGapTransport {
        fn pull(&self, _request: &BatchPullRequest) -> Result<BatchPullResponse> {
            let call = self.pull_calls.get() + 1;
            self.pull_calls.set(call);
            if call > 4 {
                return Err(anyhow!(
                    "materialization-gap regression: pull loop did not terminate"
                ));
            }
            Ok(BatchPullResponse {
                documents: vec![PulledCrdtDocument {
                    document: self.document,
                    kind: SyncDocumentKind::Scheme,
                    seq: 1,
                    epoch: 0,
                    state_v1: self.state_v1.clone(),
                    state_v1_is_delta: false,
                }],
                known_documents: Some(HashMap::from([(self.document, 1)])),
                ..BatchPullResponse::default()
            })
        }

        fn push(&self, _request: &BatchPushRequest) -> Result<BatchPushResponse> {
            Ok(BatchPushResponse::default())
        }
    }

    /// Returns a stale workspace index that predates a scheme present only in
    /// the caller's plain workspace. This is the exact ordering hazard where a
    /// pull used to materialize the stale index and erase the local scheme.
    struct StaleWorkspaceIndexTransport {
        workspace_document: DocumentId,
        state_v1: Vec<u8>,
    }

    impl SyncTransport for StaleWorkspaceIndexTransport {
        fn pull(&self, _request: &BatchPullRequest) -> Result<BatchPullResponse> {
            Ok(BatchPullResponse {
                documents: vec![PulledCrdtDocument {
                    document: self.workspace_document,
                    kind: SyncDocumentKind::PersonalWorkspace,
                    seq: 1,
                    epoch: 0,
                    state_v1: self.state_v1.clone(),
                    state_v1_is_delta: false,
                }],
                known_documents: Some(HashMap::from([(self.workspace_document, 1)])),
                ..BatchPullResponse::default()
            })
        }

        fn push(&self, _request: &BatchPushRequest) -> Result<BatchPushResponse> {
            Ok(BatchPushResponse::default())
        }
    }

    struct RemoteSchemeTransport {
        index_document: DocumentId,
        scheme_document: DocumentId,
        state_v1: Vec<u8>,
    }

    impl SyncTransport for RemoteSchemeTransport {
        fn pull(&self, _request: &BatchPullRequest) -> Result<BatchPullResponse> {
            Ok(BatchPullResponse {
                documents: vec![PulledCrdtDocument {
                    document: self.scheme_document,
                    kind: SyncDocumentKind::Scheme,
                    seq: 2,
                    epoch: 0,
                    state_v1: self.state_v1.clone(),
                    state_v1_is_delta: false,
                }],
                known_documents: Some(HashMap::from([
                    (self.index_document, 1),
                    (self.scheme_document, 2),
                ])),
                ..BatchPullResponse::default()
            })
        }

        fn push(&self, _request: &BatchPushRequest) -> Result<BatchPushResponse> {
            Ok(BatchPushResponse::default())
        }
    }

    /// Production-fuzz seed 4: a device whose plain Daily file was ahead of its
    /// CRDT (an edit saved before a crash) ran the pre-pull content repair, and
    /// the post-pull repair then re-wrote that scheme from the pre-pull plain
    /// copy after the pull had delivered another device's new lines. A scheme
    /// write makes the document match exactly, so those lines were tombstoned
    /// locally and on the server. Only the locally-ahead lines may be re-written.
    #[test]
    fn post_pull_repair_keeps_lines_the_pull_delivered() {
        let replica = ReplicaId::new();
        let mut base = Workspace::new();
        let mut scheme = knotq_model::Scheme::new("Day", 0);
        scheme.items.push(knotq_model::Item::new("old"));
        let scheme_id = scheme.id;
        let edited_item = scheme.items[0].id;
        base.schemes.insert(scheme_id, scheme);
        let root = base.root;
        base.folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(knotq_model::NodeRef::Scheme(scheme_id));
        base.ensure_sync_metadata();
        let index = base.sync.id;
        let scheme_document = base.scheme_sync[&scheme_id].id;
        let base_states = WorkspaceCrdtDocuments::try_new(&base)
            .unwrap()
            .document_states();

        // Another device appends a line; the server holds that state.
        let mut remote_docs =
            WorkspaceCrdtDocuments::from_states(&base, ReplicaId::new(), &base_states).unwrap();
        let mut remote = base.clone();
        let added = knotq_model::Item::new("from another device");
        let added_id = added.id;
        remote
            .schemes
            .get_mut(&scheme_id)
            .unwrap()
            .items
            .push(added);
        let appended = remote_docs.sync_changes(
            &remote,
            &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
        );
        assert!(appended.is_ok(), "{:?}", appended.errors);
        let server_scheme_state = remote_docs.document_states()[&scheme_document].to_vec();

        // This device's plain file is ahead of its CRDT, so the pre-pull content
        // repair fires for the same scheme.
        let mut local_crdt =
            WorkspaceCrdtDocuments::from_states(&base, replica, &base_states).unwrap();
        let mut plain = base.clone();
        plain.schemes.get_mut(&scheme_id).unwrap().items[0].set_text("edited locally");
        let mut local_state = LocalSyncState::default();
        local_state.mark_pulled(index, SyncDocumentKind::PersonalWorkspace, 1, 0);
        local_state.mark_pulled(scheme_document, SyncDocumentKind::Scheme, 1, 0);

        let transport = RemoteSchemeTransport {
            index_document: index,
            scheme_document,
            state_v1: server_scheme_state.clone(),
        };
        let outcome = batch_pull_and_apply(
            &transport,
            &mut local_crdt,
            &mut local_state,
            plain,
            replica,
        )
        .expect("pull");

        let items = &outcome.workspace.schemes[&scheme_id].items;
        assert!(
            items.iter().any(|item| item.id == added_id),
            "the line the pull delivered was removed locally"
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == edited_item)
                .map(|item| item.text()),
            Some("edited locally".to_string())
        );

        // The repair's queued edits must not remove it on the server either.
        let queued: Vec<Vec<u8>> = local_state
            .pending
            .iter()
            .filter(|edit| edit.document == scheme_document)
            .map(|edit| edit.update_v1.clone())
            .collect();
        let on_server = crate::testing::merge_state(&server_scheme_state, &queued);
        let mut server_states: HashMap<DocumentId, Vec<u8>> = base_states
            .iter()
            .map(|(document, state)| (*document, state.to_vec()))
            .collect();
        server_states.insert(scheme_document, on_server);
        let server_docs =
            WorkspaceCrdtDocuments::from_states(&base, ReplicaId::new(), &server_states).unwrap();
        let server = server_docs
            .materialized_workspace_repair(&base, &|_| false)
            .unwrap();
        let server_items = &server.schemes[&scheme_id].items;
        assert!(
            server_items.iter().any(|item| item.id == added_id),
            "the repair's push removes the delivered line on the server"
        );
        assert_eq!(
            server_items
                .iter()
                .find(|item| item.id == edited_item)
                .map(|item| item.text()),
            Some("edited locally".to_string())
        );
    }

    /// Production-fuzz seed 3: a device's queued workspace-index edits (a
    /// folder rename) were dropped by the pre-pull repair and replaced with a
    /// diff taken against the CRDT that already held them — a near-empty,
    /// delete-only update. The server accepted it and silently discarded it,
    /// the device cleared the real edits as pushed, and the rename never
    /// reached any other device. The repair must queue the full index instead.
    #[test]
    fn pre_pull_repair_keeps_the_index_edits_it_replaces() {
        let replica = ReplicaId::new();
        let mut server_workspace = Workspace::new();
        let folder = knotq_model::Folder {
            id: knotq_model::FolderId::new(),
            name: "Before".to_string(),
            parent: Some(server_workspace.root),
            children: Vec::new(),
            expanded: true,
        };
        let folder_id = folder.id;
        let root = server_workspace.root;
        server_workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(knotq_model::NodeRef::Folder(folder_id));
        server_workspace.folders.insert(folder_id, folder);
        server_workspace.ensure_sync_metadata();
        let index = server_workspace.sync.id;
        let mut crdt = WorkspaceCrdtDocuments::try_new(&server_workspace).unwrap();
        let server_state = crdt.document_states()[&index].to_vec();

        // The device renames the folder: the edit is in its CRDT and queued.
        let mut renamed = server_workspace.clone();
        renamed.folders.get_mut(&folder_id).unwrap().name = "After".to_string();
        let rename = crdt.sync_changes(&renamed, &WorkspaceCrdtChangeSet::default().workspace());
        assert!(rename.is_ok(), "{:?}", rename.errors);
        let mut local_state = LocalSyncState::default();
        local_state.mark_pulled(index, SyncDocumentKind::PersonalWorkspace, 1, 0);
        queue_crdt_updates(&mut local_state, &renamed, replica, rename.updates);
        assert!(local_state
            .pending
            .iter()
            .any(|edit| edit.kind == SyncDocumentKind::PersonalWorkspace));

        // The plain workspace is ahead of the CRDT's folder records (a folder
        // the CRDT never received), which triggers the repair.
        let mut plain = renamed.clone();
        let extra = knotq_model::Folder {
            id: knotq_model::FolderId::new(),
            name: "Plain only".to_string(),
            parent: Some(root),
            children: Vec::new(),
            expanded: true,
        };
        let extra_id = extra.id;
        plain
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(knotq_model::NodeRef::Folder(extra_id));
        plain.folders.insert(extra_id, extra);
        let repair =
            queue_local_only_documents_before_pull(&mut crdt, &mut local_state, &plain, replica);
        assert!(
            repair.is_some(),
            "the folder-record mismatch must trigger the repair"
        );

        let queued: Vec<Vec<u8>> = local_state
            .pending
            .iter()
            .filter(|edit| edit.document == index)
            .map(|edit| edit.update_v1.clone())
            .collect();
        let on_server = crate::testing::merge_state(&server_state, &queued);
        let server_docs = WorkspaceCrdtDocuments::from_states(
            &plain,
            replica,
            &HashMap::from([(index, on_server)]),
        )
        .unwrap();
        assert!(
            server_docs.workspace_folder_records_match(&plain).unwrap(),
            "the server's index after the repair's push is missing the queued rename or the repaired folder"
        );
    }

    #[test]
    fn pre_pull_repair_preserves_plain_workspace_scheme_against_stale_index() {
        let stale_workspace = Workspace::new();
        let mut local_workspace = stale_workspace.clone();
        let mut scheme = knotq_model::Scheme::new("Local-only", 0);
        scheme.items.push(knotq_model::Item::new("must survive"));
        let scheme_id = scheme.id;
        local_workspace.schemes.insert(scheme_id, scheme);
        local_workspace.ensure_sync_metadata();
        let scheme_document = local_workspace.scheme_sync[&scheme_id].id;
        let folder = knotq_model::Folder {
            id: knotq_model::FolderId::new(),
            name: "Local-only folder".to_string(),
            parent: Some(local_workspace.root),
            children: Vec::new(),
            expanded: true,
        };
        let folder_id = folder.id;
        local_workspace
            .folders
            .get_mut(&local_workspace.root)
            .unwrap()
            .children
            .push(knotq_model::NodeRef::Folder(folder_id));
        local_workspace.folders.insert(folder_id, folder);

        // The local CRDT was restored from the older workspace before the
        // scheme was created, while the plain workspace already contains it.
        let stale_crdt = WorkspaceCrdtDocuments::try_new(&stale_workspace).unwrap();
        let workspace_document = stale_workspace.sync.id;
        let stale_workspace_state = stale_crdt.document_states()[&workspace_document].to_vec();
        let mut local_crdt = stale_crdt;
        assert!(!local_crdt.known_document_ids().contains(&scheme_document));

        let transport = StaleWorkspaceIndexTransport {
            workspace_document,
            state_v1: stale_workspace_state,
        };
        let mut local_state = LocalSyncState::default();
        // A CRDT restored from an older state implies a device that has synced
        // with this server before — the repair's precondition. (A cursor on an
        // unrelated document, so the stale index below is still applied.)
        local_state.mark_pulled(
            knotq_model::DocumentId::new(),
            knotq_model::SyncDocumentKind::Scheme,
            1,
            0,
        );

        let outcome = batch_pull_and_apply(
            &transport,
            &mut local_crdt,
            &mut local_state,
            local_workspace,
            ReplicaId::new(),
        )
        .expect("a stale remote index must not erase a local-only scheme");

        assert!(outcome.workspace.schemes.contains_key(&scheme_id));
        assert_eq!(
            outcome.workspace.folders[&folder_id].name,
            "Local-only folder"
        );
        assert_eq!(
            outcome.workspace.schemes[&scheme_id].items[0].text(),
            "must survive"
        );
        assert!(
            outcome
                .locally_repaired_documents
                .contains(&scheme_document),
            "the missing local CRDT document must be explicitly repaired"
        );
        assert!(
            local_state
                .pending
                .iter()
                .any(|edit| edit.document == scheme_document),
            "the recovered scheme must be queued for the server"
        );
    }

    #[test]
    fn deferred_integrity_pull_rechecks_once_after_changed_documents() {
        let mut workspace = Workspace::new();
        let mut scheme = knotq_model::Scheme::new("Plan", 0);
        scheme.items.push(knotq_model::Item::new("remote"));
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let document = workspace.scheme_sync[&scheme_id].id;
        let crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        let state_v1 = crdt.document_states()[&document].to_vec();
        let transport = DeferredIntegrityTransport {
            requests: RefCell::new(Vec::new()),
            responses: RefCell::new(VecDeque::from([
                BatchPullResponse {
                    documents: vec![PulledCrdtDocument {
                        document,
                        kind: SyncDocumentKind::Scheme,
                        seq: 1,
                        epoch: 0,
                        state_v1,
                        state_v1_is_delta: false,
                    }],
                    known_documents: Some(HashMap::from([(document, 1)])),
                    integrity_check_deferred: true,
                    ..BatchPullResponse::default()
                },
                BatchPullResponse {
                    known_documents: Some(HashMap::from([(document, 1)])),
                    integrity_mismatches: Some(Vec::new()),
                    ..BatchPullResponse::default()
                },
            ])),
        };
        let mut local_crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        let mut local_state = LocalSyncState::default();

        batch_pull_and_apply(
            &transport,
            &mut local_crdt,
            &mut local_state,
            workspace,
            ReplicaId::new(),
        )
        .unwrap();

        let requests = transport.requests.into_inner();
        assert_eq!(
            requests.len(),
            2,
            "changed page must get one caught-up proof retry"
        );
        assert!(!requests[0].integrity_state_vectors.is_empty());
        assert!(!requests[1].integrity_state_vectors.is_empty());
    }

    #[test]
    fn materialization_gap_finishes_so_manual_sync_can_run_again() {
        // Build a valid server scheme snapshot, then give the client only the
        // durable workspace-index binding. This is intentionally an older or
        // partially restored workspace: the scheme is named in `scheme_sync`,
        // but its local CRDT document cannot be materialized.
        let mut indexed_workspace = Workspace::new();
        let mut scheme = knotq_model::Scheme::new("Older scheme", 0);
        scheme.items.push(knotq_model::Item::new("remote"));
        let scheme_id = scheme.id;
        indexed_workspace.schemes.insert(scheme_id, scheme);
        indexed_workspace.ensure_sync_metadata();
        let document = indexed_workspace.scheme_sync[&scheme_id].id;
        let server_state = WorkspaceCrdtDocuments::try_new(&indexed_workspace)
            .unwrap()
            .document_states()[&document]
            .to_vec();

        let mut stale_workspace = indexed_workspace;
        stale_workspace.schemes.remove(&scheme_id);
        let mut local_crdt = WorkspaceCrdtDocuments::try_new(&stale_workspace).unwrap();
        assert!(!local_crdt.known_document_ids().contains(&document));

        let transport = RepeatingMaterializationGapTransport {
            pull_calls: Cell::new(0),
            document,
            state_v1: server_state,
        };
        let mut local_state = LocalSyncState::default();

        let outcome = batch_pull_and_apply(
            &transport,
            &mut local_crdt,
            &mut local_state,
            stale_workspace,
            ReplicaId::new(),
        )
        .expect("a non-materializable scheme must not wedge the pull");

        assert_eq!(
            outcome.pull_requests, 1,
            "manual Sync now must get a completed pull to run after this one"
        );
        assert_eq!(transport.pull_calls.get(), 1);
        assert!(outcome
            .skipped
            .iter()
            .any(|skipped| skipped.document == document && skipped.deferred));
        assert_eq!(
            local_state.document_cursors[&document].last_pulled_sequence, 1,
            "the failed materialization must advance the cursor instead of resetting it"
        );
    }

    #[test]
    fn ordinary_pull_does_not_infer_delta_hint_without_checkpoint() {
        let mut workspace = Workspace::new();
        let mut scheme = knotq_model::Scheme::new("Plan", 0);
        scheme.items.push(knotq_model::Item::new("local"));
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let document = workspace.scheme_sync[&scheme_id].id;
        let transport = DeferredIntegrityTransport {
            requests: RefCell::new(Vec::new()),
            responses: RefCell::new(VecDeque::from([BatchPullResponse {
                known_documents: Some(HashMap::from([(document, 1)])),
                ..BatchPullResponse::default()
            }])),
        };
        let mut local_crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        let mut local_state = LocalSyncState::default();
        // Model an install upgraded from the cursor-only protocol: it knows the
        // server sequence but has not yet populated the new vector cache. A
        // cursor alone is not enough to prove that the local CRDT belongs to
        // the same server/account lineage, so this first pull must be full.
        local_state.mark_pulled(document, SyncDocumentKind::Scheme, 1, 0);

        batch_pull_and_apply(
            &transport,
            &mut local_crdt,
            &mut local_state,
            workspace,
            ReplicaId::new(),
        )
        .unwrap();

        let requests = transport.requests.into_inner();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].state_vectors.is_empty());
        assert!(!local_state.integrity_state_vectors.contains_key(&document));
    }

    #[test]
    fn scoped_integrity_ignores_legacy_server_mismatches_outside_scope() {
        let mut workspace = Workspace::new();
        let mut scheme = knotq_model::Scheme::new("Plan", 0);
        scheme.items.push(knotq_model::Item::new("local"));
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let document = workspace.scheme_sync[&scheme_id].id;
        let unrelated = DocumentId::new();
        let crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        let transport = DeferredIntegrityTransport {
            requests: RefCell::new(Vec::new()),
            responses: RefCell::new(VecDeque::from([BatchPullResponse {
                // A pre-scoped-proof backend reports omitted heads as
                // mismatches. The client must not reset/re-pull this document.
                integrity_mismatches: Some(vec![unrelated]),
                known_documents: Some(HashMap::from([(document, 1)])),
                ..BatchPullResponse::default()
            }])),
        };
        let mut local_crdt = crdt;
        let mut local_state = LocalSyncState::default();
        let scope = HashSet::from([document]);

        batch_pull_and_apply_with_integrity_documents(
            &transport,
            &mut local_crdt,
            &mut local_state,
            workspace,
            ReplicaId::new(),
            true,
            Some(&scope),
        )
        .unwrap();

        assert!(local_state.document_cursors.is_empty());
        assert_eq!(transport.requests.into_inner().len(), 1);
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

        let (request, acks) = build_push_request(&state, replica_id, &schedule(), false).unwrap();

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

        let (request, acks) = build_push_request(&state, replica_id, &schedule(), false).unwrap();

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

        let (request, acks) = build_push_request(&state, replica_id, &schedule(), false).unwrap();

        assert_eq!(request.documents.len(), 1);
        assert_eq!(request.documents[0].document, first);
        assert!(raw_request_bytes(&request) <= PUSH_MAX_RAW_UPDATE_BYTES_PER_REQUEST);
        assert_eq!(acks[0].through_local_sequence, 1);
    }

    #[test]
    fn accepted_push_records_server_head_for_post_push_cursor_optimization() {
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let document = DocumentId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace_id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.push_pending(pending(workspace_id, replica_id, document, 7, 1));
        let workspace = Workspace::new();
        let mut crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        let transport = PushAckTransport {
            server_sequence: 42,
        };
        let mut pushed = Vec::new();

        batch_push_pending(
            &transport,
            &mut state,
            replica_id,
            &schedule(),
            false,
            &mut pushed,
            &mut crdt,
            &workspace,
        )
        .unwrap();

        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].document, document);
        assert_eq!(pushed[0].kind, SyncDocumentKind::Scheme);
        assert_eq!(pushed[0].through_local_sequence, 7);
        assert_eq!(pushed[0].server_sequence, 42);
        assert!(state.pending.is_empty());
    }

    struct FailingPushTransport;

    impl SyncTransport for FailingPushTransport {
        fn pull(&self, _request: &BatchPullRequest) -> Result<BatchPullResponse> {
            Ok(BatchPullResponse::default())
        }

        fn push(&self, _request: &BatchPushRequest) -> Result<BatchPushResponse> {
            Err(anyhow!("network unavailable"))
        }
    }

    #[test]
    fn full_reseed_obligation_clears_only_once_the_push_queue_fully_drains() {
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let document = DocumentId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace_id),
            replica_id: Some(replica_id),
            reseed_all_documents: true,
            ..LocalSyncState::default()
        };
        state.push_pending(pending(workspace_id, replica_id, document, 1, 8));
        let workspace = Workspace::new();
        let mut crdt = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();

        // A push that fails must leave the guard armed: clearing it here would
        // let the next attempt's pre-pull local repair run as though this device
        // had already re-seeded the new account, letting a stale/source-account
        // tombstone overwrite destination data.
        let failing = FailingPushTransport;
        let mut pushed = Vec::new();
        assert!(batch_push_pending(
            &failing,
            &mut state,
            replica_id,
            &schedule(),
            false,
            &mut pushed,
            &mut crdt,
            &workspace,
        )
        .is_err());
        assert!(state.needs_full_reseed());
        assert!(!state.pending.is_empty());

        // Once every queued edit is actually accepted by the server, the
        // obligation is satisfied and the guard drops.
        let transport = PushAckTransport { server_sequence: 9 };
        batch_push_pending(
            &transport,
            &mut state,
            replica_id,
            &schedule(),
            false,
            &mut pushed,
            &mut crdt,
            &workspace,
        )
        .unwrap();
        assert!(state.pending.is_empty());
        assert!(!state.needs_full_reseed());
    }
}
