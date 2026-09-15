//! In-memory sync backend for tests and fuzzers — not used by any client.
//!
//! [`MemoryServer`] implements the real [`SyncTransport`] against the production
//! worker's merged-state model: one merged Yjs `state_v1` per document, bumped by
//! a `seq` on each push, with the worker's all-or-nothing batch validation. It
//! lives in the crate (rather than one test directory) so every client's
//! production-path fuzzer — the shared engine's, the desktop's, mobile's — syncs
//! against the same server model.
//!
//! ## Backend atomicity semantics (from `backend/cloudflare/src/index.ts`)
//!
//! `handleSyncPush` iterates over documents inside a single
//! `this.state.storage.transactionSync(() => { … })` call. Any throw inside that
//! closure — including `crdt_schema_invalid` for any document in the batch —
//! aborts the whole transaction: no documents from that batch are persisted.
//! [`MemoryServer::push`] validates every document before writing any.

use std::cell::RefCell;
use std::collections::HashMap;

use anyhow::anyhow;
use knotq_model::{DocumentId, SyncDocumentKind};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::{
    validate_crdt_update_sequence, BatchPullRequest, BatchPullResponse, BatchPushRequest,
    BatchPushResponse, DocumentPullStateVector, PulledCrdtDocument, PushedCrdtDocument,
    SyncPushRejected, SyncTransport, WorkspaceCrdtDocuments, MAX_SYNC_MEDIA_BYTES,
};

/// Media asset key: (document_id, image_name) → bytes.
type MediaKey = (DocumentId, String);

#[derive(Default)]
pub struct MemoryServer {
    documents: RefCell<HashMap<DocumentId, ServerDocument>>,
    /// In-memory stand-in for the R2 object store. Mirrors the backend's per-asset
    /// limit (`MAX_SYNC_MEDIA_BYTES`).
    media: RefCell<HashMap<MediaKey, Vec<u8>>>,
    counters: RefCell<ServerCounters>,
    /// When set, the next push call returns this rejection code unconditionally
    /// and clears it (one-shot).
    reject_next_push: RefCell<Option<String>>,
    /// How many upcoming pulls fail as if the network dropped.
    failing_pulls: RefCell<usize>,
    /// How many upcoming pushes are applied by the server but whose response
    /// never reaches the client — the lost-acknowledgement case, where the
    /// client must re-push edits the server already holds.
    lost_push_responses: RefCell<usize>,
}

#[derive(Default)]
struct ServerCounters {
    pull_calls: usize,
    push_calls: usize,
    /// How many times `push` organically rejected a batch with `crdt_schema_invalid`.
    /// Excludes the one-shot `reject_next_push` fault injection.
    schema_invalid_rejections: usize,
    /// Documents flagged by the most recent integrity-bearing pull.
    last_integrity_mismatches: usize,
    /// Documents served as state-vector deltas by the pull(s) so far.
    delta_pull_documents: usize,
    /// Subset of the mismatches where the client DID submit a state vector but it
    /// disagreed with the server's re-derived one.
    last_integrity_vector_disagreements: usize,
}

struct ServerDocument {
    kind: SyncDocumentKind,
    seq: u64,
    epoch: u64,
    state_v1: Vec<u8>,
}

/// The error a dropped connection surfaces as.
#[derive(Debug)]
pub struct MemoryServerNetworkError;

impl std::fmt::Display for MemoryServerNetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("memory server: connection dropped")
    }
}

impl std::error::Error for MemoryServerNetworkError {}

impl MemoryServer {
    pub fn pull_calls(&self) -> usize {
        self.counters.borrow().pull_calls
    }

    pub fn push_calls(&self) -> usize {
        self.counters.borrow().push_calls
    }

    /// Number of batches organically rejected with `crdt_schema_invalid`.
    pub fn schema_invalid_rejections(&self) -> usize {
        self.counters.borrow().schema_invalid_rejections
    }

    pub fn document_count(&self) -> usize {
        self.documents.borrow().len()
    }

    // --- network faults ---------------------------------------------------------

    /// Fail the next `count` pulls as if the connection dropped.
    pub fn fail_next_pulls(&self, count: usize) {
        *self.failing_pulls.borrow_mut() += count;
    }

    /// Apply the next `count` pushes but lose their responses: the server state
    /// advances while the client sees an error.
    pub fn lose_next_push_responses(&self, count: usize) {
        *self.lost_push_responses.borrow_mut() += count;
    }

    // --- in-memory media store --------------------------------------------------

    /// Mirrors `PUT /v1/sync/documents/{document}/media/{image_name}`.
    pub fn upload_media(
        &self,
        document: DocumentId,
        image_name: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "media asset {} exceeds the {} byte limit ({} bytes)",
                image_name,
                MAX_SYNC_MEDIA_BYTES,
                bytes.len(),
            ));
        }
        self.media
            .borrow_mut()
            .insert((document, image_name.to_string()), bytes);
        Ok(())
    }

    /// Mirrors `GET /v1/sync/documents/{document}/media/{image_name}`.
    pub fn download_media(&self, document: DocumentId, image_name: &str) -> Option<Vec<u8>> {
        self.media
            .borrow()
            .get(&(document, image_name.to_string()))
            .cloned()
    }

    pub fn media_asset_count(&self) -> usize {
        self.media.borrow().len()
    }

    /// Arm a one-shot `crdt_schema_invalid` rejection of the next push.
    pub fn reject_next_push_with_schema_invalid(&self) {
        self.reject_next_push_with_code("crdt_schema_invalid");
    }

    /// Arm a one-shot rejection of the next push with an arbitrary backend code.
    pub fn reject_next_push_with_code(&self, code: &str) {
        *self.reject_next_push.borrow_mut() = Some(code.to_string());
    }

    /// Inject a valid scheme content document with no workspace-index entry — an
    /// orphan content document. Returns its id.
    pub fn inject_orphan_scheme_document(&self, scheme: &knotq_model::Scheme) -> DocumentId {
        let mut workspace = knotq_model::Workspace::new();
        workspace.ensure_sync_metadata();
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme.clone());
        workspace.ensure_sync_metadata();
        let doc_id = workspace
            .scheme_sync
            .get(&scheme_id)
            .expect("scheme sync meta")
            .id;
        let updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace).updates;
        let scheme_update = updates
            .into_iter()
            .find(|u| u.document == doc_id)
            .expect("scheme update");
        self.documents.borrow_mut().insert(
            doc_id,
            ServerDocument {
                kind: SyncDocumentKind::Scheme,
                seq: 1,
                epoch: 0,
                state_v1: scheme_update.update_v1,
            },
        );
        doc_id
    }

    /// Server-side effect of an accepted `POST /v1/sync/squash`: replace the
    /// stored state with the history-free rebuild, bumping seq AND epoch.
    pub fn squash_document(&self, document: DocumentId, state_v1: Vec<u8>) -> (u64, u64) {
        let mut documents = self.documents.borrow_mut();
        let doc = documents.get_mut(&document).expect("squash target exists");
        doc.state_v1 = state_v1;
        doc.seq += 1;
        doc.epoch += 1;
        (doc.seq, doc.epoch)
    }

    /// `POST /v1/sync/squash` with the backend's head check: accepted only when
    /// the proposal was built against the document's current head.
    pub fn try_squash_document(
        &self,
        document: DocumentId,
        expected_seq: u64,
        expected_epoch: u64,
        state_v1: Vec<u8>,
    ) -> anyhow::Result<(u64, u64)> {
        let head = self.document_head(document);
        if head != Some((expected_seq, expected_epoch)) {
            return Err(anyhow::Error::new(SyncPushRejected {
                code: "squash_head_moved".to_string(),
            }));
        }
        Ok(self.squash_document(document, state_v1))
    }

    /// Current (seq, epoch) head of a stored document.
    pub fn document_head(&self, document: DocumentId) -> Option<(u64, u64)> {
        self.documents
            .borrow()
            .get(&document)
            .map(|doc| (doc.seq, doc.epoch))
    }

    /// Server-side effect of the at-rest compaction sweep: every stored
    /// `state_v1` is transcoded v1 -> v2 -> v1 and re-encoded, WITHOUT bumping
    /// `seq` or `epoch`.
    pub fn run_compaction(&self) {
        let mut documents = self.documents.borrow_mut();
        for doc in documents.values_mut() {
            let Ok(update) = Update::decode_v1(&doc.state_v1) else {
                continue;
            };
            let v2 = update.encode_v2();
            let Ok(back) = Update::decode_v2(&v2) else {
                continue;
            };
            let rebuilt = Doc::new();
            {
                let mut txn = rebuilt.transact_mut();
                if txn.apply_update(back).is_err() {
                    continue;
                }
            }
            doc.state_v1 = rebuilt.transact().encode_diff_v1(&StateVector::default());
        }
    }

    pub fn last_integrity_mismatch_count(&self) -> usize {
        self.counters.borrow().last_integrity_mismatches
    }

    pub fn last_integrity_vector_disagreement_count(&self) -> usize {
        self.counters.borrow().last_integrity_vector_disagreements
    }

    pub fn delta_pull_documents(&self) -> usize {
        self.counters.borrow().delta_pull_documents
    }

    /// Replace the personal workspace document's state with undecodable bytes.
    pub fn corrupt_workspace_document(&self, workspace_doc_id: DocumentId) {
        let mut documents = self.documents.borrow_mut();
        if let Some(doc) = documents.get_mut(&workspace_doc_id) {
            doc.state_v1 = vec![0xFF, 0xFE, 0xFD, 0x01, 0x02, 0x03];
            doc.seq += 1;
        }
    }

    fn take_fault(counter: &RefCell<usize>) -> bool {
        let mut remaining = counter.borrow_mut();
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        true
    }
}

/// A pull-only view of a [`MemoryServer`] that never consumes injected faults —
/// for a test's own audit of what the server holds.
pub struct MemoryServerAudit<'a>(&'a MemoryServer);

impl SyncTransport for MemoryServerAudit<'_> {
    fn pull(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        self.0.pull_documents(request)
    }

    fn push(&self, _request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        Err(anyhow!("the audit transport is read-only"))
    }
}

impl MemoryServer {
    /// Pull through this server without touching its fault injection.
    pub fn audit(&self) -> MemoryServerAudit<'_> {
        MemoryServerAudit(self)
    }
}

impl SyncTransport for MemoryServer {
    fn pull(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        self.counters.borrow_mut().pull_calls += 1;
        if Self::take_fault(&self.failing_pulls) {
            return Err(anyhow::Error::new(MemoryServerNetworkError));
        }
        self.pull_documents(request)
    }

    fn push(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        self.push_documents(request)
    }
}

impl MemoryServer {
    fn pull_documents(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        let documents = self.documents.borrow();
        let requested_vectors: HashMap<DocumentId, &DocumentPullStateVector> = request
            .state_vectors
            .iter()
            .map(|entry| (entry.document, entry))
            .collect();
        let mut delta_pull_documents = 0usize;
        let pulled: Vec<PulledCrdtDocument> = documents
            .iter()
            .filter(|(id, doc)| doc.seq > request.cursors.get(*id).copied().unwrap_or(0))
            .map(|(id, doc)| {
                let (state_v1, state_v1_is_delta) = requested_vectors
                    .get(id)
                    .filter(|hint| hint.epoch == doc.epoch)
                    .and_then(|hint| {
                        let update = Update::decode_v1(&doc.state_v1).ok()?;
                        let state_vector = StateVector::decode_v1(&hint.state_vector_v1).ok()?;
                        let server_doc = Doc::new();
                        {
                            let mut txn = server_doc.transact_mut();
                            txn.apply_update(update).ok()?;
                        }
                        let delta = server_doc.transact().encode_diff_v1(&state_vector);
                        Some((delta, true))
                    })
                    .unwrap_or_else(|| (doc.state_v1.clone(), false));
                if state_v1_is_delta {
                    delta_pull_documents += 1;
                }
                PulledCrdtDocument {
                    document: *id,
                    kind: doc.kind,
                    seq: doc.seq,
                    epoch: doc.epoch,
                    state_v1,
                    state_v1_is_delta,
                }
            })
            .collect();
        self.counters.borrow_mut().delta_pull_documents += delta_pull_documents;
        let known_documents = documents.iter().map(|(id, doc)| (*id, doc.seq)).collect();

        // Mirror the backend's pull integrity check: only on a caught-up pull that
        // carries state-vector proofs, re-derive each stored document's state
        // vector and compare it to what the client submitted.
        let integrity_mismatches = if pulled.is_empty()
            && !request.integrity_state_vectors.is_empty()
        {
            let submitted: HashMap<DocumentId, &[u8]> = request
                .integrity_state_vectors
                .iter()
                .map(|entry| (entry.document, entry.state_vector_v1.as_slice()))
                .collect();
            let mut mismatched = Vec::new();
            let mut disagreements = 0usize;
            for (id, doc) in documents.iter() {
                let expected = Update::decode_v1(&doc.state_v1).ok().map(|update| {
                    let rebuilt = Doc::new();
                    {
                        let mut txn = rebuilt.transact_mut();
                        let _ = txn.apply_update(update);
                    }
                    let sv = rebuilt.transact().state_vector().encode_v1();
                    sv
                });
                match (submitted.get(id), expected) {
                    (Some(client_sv), Some(server_sv)) if *client_sv == server_sv.as_slice() => {}
                    (Some(_), _) => {
                        disagreements += 1;
                        mismatched.push(*id);
                    }
                    _ => mismatched.push(*id),
                }
            }
            {
                let mut counters = self.counters.borrow_mut();
                counters.last_integrity_mismatches = mismatched.len();
                counters.last_integrity_vector_disagreements = disagreements;
            }
            Some(mismatched)
        } else {
            None
        };

        Ok(BatchPullResponse {
            documents: pulled,
            known_documents: Some(known_documents),
            integrity_mismatches,
            integrity_check_deferred: false,
            notification_schedule_revision: 0,
            has_more: false,
        })
    }

    /// Mirrors `handleSyncPush` in `backend/cloudflare/src/index.ts`: validate and
    /// merge every document into a scratch buffer, then commit all or nothing.
    fn push_documents(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        self.counters.borrow_mut().push_calls += 1;

        {
            let code = self.reject_next_push.borrow_mut().take();
            if let Some(code) = code {
                return Err(anyhow::Error::new(SyncPushRejected { code }));
            }
        }

        let mut documents = self.documents.borrow_mut();

        struct ScratchEntry {
            document: DocumentId,
            kind: SyncDocumentKind,
            new_state: Vec<u8>,
            new_seq: u64,
            epoch: u64,
            accepted: usize,
        }
        let mut scratch: Vec<ScratchEntry> = Vec::with_capacity(request.documents.len());

        for doc in &request.documents {
            let existing = documents.get(&doc.document);
            if let Some(entry) = existing {
                if entry.epoch != doc.epoch {
                    return Err(anyhow::Error::new(SyncPushRejected {
                        code: "document_epoch_stale".to_string(),
                    }));
                }
                if entry.kind != doc.kind {
                    return Err(anyhow!(
                        "sync backend rejected request: document_kind_mismatch for {}",
                        doc.document
                    ));
                }
            }
            let base = existing.map(|e| e.state_v1.as_slice()).unwrap_or(&[]);
            let mut chain: Vec<&[u8]> = Vec::new();
            if !base.is_empty() {
                chain.push(base);
            }
            chain.extend(doc.updates.iter().map(|u| u.as_slice()));
            if let Err(err) = validate_crdt_update_sequence(doc.kind, chain.iter().copied()) {
                eprintln!(
                    "[MemoryServer] crdt_schema_invalid for {:?} {} (had_base={}, updates={}): {err:#}",
                    doc.kind,
                    doc.document,
                    !base.is_empty(),
                    doc.updates.len(),
                );
                self.counters.borrow_mut().schema_invalid_rejections += 1;
                return Err(anyhow::Error::new(SyncPushRejected {
                    code: "crdt_schema_invalid".to_string(),
                }));
            }
            let new_state = merge_state(base, &doc.updates);
            let new_seq = existing.map(|e| e.seq).unwrap_or(0) + 1;
            let epoch = existing.map(|e| e.epoch).unwrap_or(doc.epoch);
            scratch.push(ScratchEntry {
                document: doc.document,
                kind: doc.kind,
                new_state,
                new_seq,
                epoch,
                accepted: doc.updates.len(),
            });
        }

        let mut out = Vec::with_capacity(scratch.len());
        for entry in scratch {
            documents.insert(
                entry.document,
                ServerDocument {
                    kind: entry.kind,
                    seq: entry.new_seq,
                    epoch: entry.epoch,
                    state_v1: entry.new_state,
                },
            );
            out.push(PushedCrdtDocument {
                document: entry.document,
                seq: entry.new_seq,
                accepted: entry.accepted,
            });
        }
        drop(documents);

        if Self::take_fault(&self.lost_push_responses) {
            return Err(anyhow::Error::new(MemoryServerNetworkError));
        }

        Ok(BatchPushResponse {
            documents: out,
            notification_schedule_revision: 0,
            background_pushes_enqueued: 0,
        })
    }
}

/// Merge a stored merged state plus a batch of v1 updates into a new merged state,
/// exactly as the worker's `validateAndCompactCrdtUpdates` does.
pub fn merge_state(base: &[u8], updates: &[Vec<u8>]) -> Vec<u8> {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        if !base.is_empty() {
            txn.apply_update(Update::decode_v1(base).expect("decode base"))
                .expect("apply base");
        }
        for update in updates {
            txn.apply_update(Update::decode_v1(update).expect("decode update"))
                .expect("apply update");
        }
    }
    let encoded = doc.transact().encode_diff_v1(&StateVector::default());
    encoded
}
