//! Test server — implements the real `SyncTransport` against the merged-state model.
use super::*;

/// Media asset key: (document_id, image_name) → bytes.
type MediaKey = (DocumentId, String);

#[derive(Default)]
pub struct TestServer {
    documents: RefCell<HashMap<DocumentId, ServerDocument>>,
    /// In-memory stand-in for the R2 object store.  Mirrors the backend's per-asset
    /// 3 MiB limit (`MAX_SYNC_MEDIA_BYTES`).  Upload/download are separate methods
    /// rather than part of `SyncTransport` (the trait covers only CRDT push/pull);
    /// test helpers call them directly.
    media: RefCell<HashMap<MediaKey, Vec<u8>>>,
    counters: RefCell<ServerCounters>,
    /// When set, the next push call returns this rejection code unconditionally
    /// and clears it (one-shot).
    reject_next_push: RefCell<Option<String>>,
}

#[derive(Default)]
struct ServerCounters {
    pull_calls: usize,
    push_calls: usize,
    /// How many times `push` organically rejected a batch with `crdt_schema_invalid`
    /// (a baseless bare delta that reconstructs a schema-less document). Excludes the
    /// one-shot `reject_next_push` fault injection, so a test can assert that a clean
    /// account switch never pushes a bare delta in the first place.
    schema_invalid_rejections: usize,
}

struct ServerDocument {
    kind: SyncDocumentKind,
    seq: u64,
    state_v1: Vec<u8>,
}

impl TestServer {
    pub fn pull_calls(&self) -> usize {
        self.counters.borrow().pull_calls
    }

    pub fn push_calls(&self) -> usize {
        self.counters.borrow().push_calls
    }

    /// Number of batches organically rejected with `crdt_schema_invalid` (a baseless
    /// bare delta). Stays at 0 when the client always re-seeds a full snapshot for a
    /// document the server has no base for — the property a clean account switch must
    /// preserve. Excludes `reject_next_push_with_*` fault injection.
    pub fn schema_invalid_rejections(&self) -> usize {
        self.counters.borrow().schema_invalid_rejections
    }

    pub fn document_count(&self) -> usize {
        self.documents.borrow().len()
    }

    // --- in-memory media store -------------------------------------------------

    /// Upload a media asset.  Enforces the backend's per-asset 3 MiB cap.
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

    /// Download a media asset.  Returns `None` when not found (404 on production).
    /// Mirrors `GET /v1/sync/documents/{document}/media/{image_name}`.
    pub fn download_media(&self, document: DocumentId, image_name: &str) -> Option<Vec<u8>> {
        self.media
            .borrow()
            .get(&(document, image_name.to_string()))
            .cloned()
    }

    /// Number of distinct media assets currently stored.
    pub fn media_asset_count(&self) -> usize {
        self.media.borrow().len()
    }

    /// Arm a one-shot rejection: the next call to `push` returns `SyncPushRejected`
    /// with code `"crdt_schema_invalid"` without validating anything, leaving the
    /// server state unchanged.  Use this in tests to deterministically force the
    /// engine's self-heal path.
    pub fn reject_next_push_with_schema_invalid(&self) {
        self.reject_next_push_with_code("crdt_schema_invalid");
    }

    /// Arm a one-shot rejection with an arbitrary backend rejection code (e.g.
    /// `"updates_too_large"`), exercising the engine's generalized self-heal for
    /// non-`crdt_schema_invalid` rejections.
    pub fn reject_next_push_with_code(&self, code: &str) {
        *self.reject_next_push.borrow_mut() = Some(code.to_string());
    }

    /// Inject a valid scheme content document directly into the server without
    /// a corresponding workspace-index entry.  This simulates the production
    /// scenario where a buggy heal path on one device created an orphan content
    /// doc: the document exists on the server and clients will pull it, but no
    /// workspace index entry points to it so `apply_remote_updates` cannot route
    /// it to a local scheme.
    ///
    /// Returns the `DocumentId` that was injected so the test can verify that
    /// the pulling device skipped and advanced past it.
    pub fn inject_orphan_scheme_document(&self, scheme: &knotq_model::Scheme) -> DocumentId {
        use knotq_sync::WorkspaceCrdtDocuments;
        // Build a minimal valid scheme CRDT snapshot from the given scheme data.
        // `snapshot_updates` mints a throwaway clientID — fine for server-side
        // injection where we only care about validity, not CRDT identity.
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
                kind: knotq_model::SyncDocumentKind::Scheme,
                seq: 1,
                state_v1: scheme_update.update_v1,
            },
        );
        doc_id
    }

    /// Corrupt the personal workspace document on the server by replacing its
    /// CRDT state with garbage bytes.  Used to test that workspace-level
    /// corruption causes the pull to return Err.
    pub fn corrupt_workspace_document(&self, workspace_doc_id: DocumentId) {
        let mut documents = self.documents.borrow_mut();
        if let Some(doc) = documents.get_mut(&workspace_doc_id) {
            // Overwrite state with bytes that cannot be decoded as a valid Yrs update.
            doc.state_v1 = vec![0xFF, 0xFE, 0xFD, 0x01, 0x02, 0x03];
            doc.seq += 1;
        }
    }
}

impl SyncTransport for TestServer {
    fn pull(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        self.counters.borrow_mut().pull_calls += 1;
        let documents = self.documents.borrow();
        let pulled = documents
            .iter()
            .filter(|(id, doc)| doc.seq > request.cursors.get(*id).copied().unwrap_or(0))
            .map(|(id, doc)| PulledCrdtDocument {
                document: *id,
                kind: doc.kind,
                seq: doc.seq,
                state_v1: doc.state_v1.clone(),
            })
            .collect();
        let known_documents = documents.iter().map(|(id, doc)| (*id, doc.seq)).collect();
        Ok(BatchPullResponse {
            documents: pulled,
            known_documents: Some(known_documents),
            notification_schedule_revision: 0,
            has_more: false,
        })
    }

    /// Mirrors `handleSyncPush` in `backend/cloudflare/src/index.ts`.
    ///
    /// The real worker wraps the entire per-document loop in a single
    /// `this.state.storage.transactionSync(() => { … })`.  Any validation failure
    /// (i.e. `validateAndCompactCrdtUpdates` throwing `ApiError(400,
    /// "crdt_schema_invalid")`) aborts the whole transaction — **no documents from
    /// that batch are persisted**.  This method replicates that fully-atomic
    /// all-or-nothing semantics: it validates and merges all documents into a
    /// scratch buffer before writing a single entry to `self.documents`, and returns
    /// a typed `SyncPushRejected` error (wrapped in `anyhow::Error`) on any failure.
    fn push(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        self.counters.borrow_mut().push_calls += 1;

        // One-shot forced rejection for self-heal regression tests.
        {
            let code = self.reject_next_push.borrow_mut().take();
            if let Some(code) = code {
                return Err(anyhow::Error::new(SyncPushRejected { code }));
            }
        }

        let mut documents = self.documents.borrow_mut();

        // --- Phase 1: validate + compact every document into a scratch buffer.
        // Mirrors the `transactionSync` body; no mutation of `documents` yet.
        struct ScratchEntry {
            document: DocumentId,
            kind: SyncDocumentKind,
            new_state: Vec<u8>,
            new_seq: u64,
            accepted: usize,
        }
        let mut scratch: Vec<ScratchEntry> = Vec::with_capacity(request.documents.len());

        for doc in &request.documents {
            let existing = documents.get(&doc.document);
            if let Some(entry) = existing {
                if entry.kind != doc.kind {
                    // Mirrors the document_kind_mismatch 409 — propagate as error.
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
                // Surface the reason (mirrors the `sync.crdt.schema_invalid` log) but
                // return the same opaque error code clients receive.
                let _ = err; // logged for debugging via test output
                eprintln!(
                    "[TestServer] crdt_schema_invalid for {:?} {} (had_base={}, updates={}): {err:#}",
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
            scratch.push(ScratchEntry {
                document: doc.document,
                kind: doc.kind,
                new_state,
                new_seq,
                accepted: doc.updates.len(),
            });
        }

        // --- Phase 2: commit all validated documents atomically.
        let mut out = Vec::with_capacity(scratch.len());
        for entry in scratch {
            documents.insert(
                entry.document,
                ServerDocument {
                    kind: entry.kind,
                    seq: entry.new_seq,
                    state_v1: entry.new_state,
                },
            );
            out.push(PushedCrdtDocument {
                document: entry.document,
                seq: entry.new_seq,
                accepted: entry.accepted,
            });
        }

        Ok(BatchPushResponse {
            documents: out,
            notification_schedule_revision: 0,
            background_pushes_enqueued: 0,
        })
    }
}
