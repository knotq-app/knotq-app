//! CRDT documents backing workspace sync. The public surface (`WorkspaceCrdtDocuments`
//! and the change/outcome types) lives here; the heavy machinery is split into focused
//! submodules:
//!   - [`encoding`]       — stable client IDs, Yjs options, inline-embed serialization
//!   - [`validation`]     — schema/structure validation of workspace & scheme docs
//!   - [`scheme_content`] — the per-scheme rich-text content CRDT
//!   - [`workspace_index`]— the folder/scheme tree + sync-metadata CRDT
//!
//! Shared data carriers (the `*Snapshot`/`*Entry` structs) and schema constants stay
//! in this module so the submodules — its descendants — can use them directly.
use std::collections::{HashMap, HashSet};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use chrono::{DateTime, NaiveDate};
use knotq_model::{
    DeletedFolderOrigin, DeletedSchemeOrigin, DocumentId, Folder, FolderId, Inline, Item,
    ItemContent, ItemId, ItemMarker, NodeRef, ReplicaId, Scheme, SchemeId, SchemeSource,
    SyncDocumentKind, SyncDocumentMeta, Workspace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use yrs::updates::{decoder::Decode, encoder::Encode};
use yrs::{
    Any, ClientID, Doc, Map, MapPrelim, MapRef, OffsetKind, Options, Out, ReadTxn, StateVector,
    Text, TextPrelim, TextRef, Transact, TransactionMut, Update, WriteTxn,
};

use crate::{CrdtDocumentUpdate, StoredCrdtUpdate};

/// Backing for a document's `encode_state_v1` cache. Encoding a Yjs document's full
/// state is one of the heaviest repeated costs in a sync/save (it serializes the
/// entire document, and the workspace does it for *every* document on each run even
/// though a typical edit touches one). [`EncodeCache::get`] returns the previously
/// encoded bytes whenever the document has not changed since.
///
/// Correctness rests on the keyed update observer installed by [`EncodeCache::new`]:
/// yrs fires `observe_update_v1` on every committed change — insert *and* delete —
/// so `dirty` is set exactly when the serialized state would differ. A keyed
/// observer needs no retained `Subscription` (it lives and dies with the document),
/// keeping the document wrapper trivially constructible.
pub(crate) struct EncodeCache {
    inner: Arc<EncodeCacheState>,
}

/// The cache's mutable half, held behind an `Arc` so a [`DocumentStateHandle`]
/// can share it with a background thread. Without that, encoding a document's
/// state could only happen wherever the document lives — the UI thread.
#[derive(Default)]
struct EncodeCacheState {
    dirty: AtomicBool,
    bytes: Mutex<Option<Arc<[u8]>>>,
}

impl EncodeCache {
    /// Install the dirty-tracking observer on `doc` and return a fresh (dirty) cache.
    pub(crate) fn new(doc: &Doc) -> Self {
        let inner = Arc::new(EncodeCacheState {
            dirty: AtomicBool::new(true),
            bytes: Mutex::new(None),
        });
        let flag = Arc::clone(&inner);
        let _ = doc.observe_update_v1_with("knotq_encode_cache", move |_txn, _evt| {
            flag.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        Self { inner }
    }

    /// A handle that can produce this document's state from another thread.
    ///
    /// The `Doc` is `#[repr(transparent)]` over a shared, lock-guarded store, so
    /// cloning one is a handle clone rather than a copy of the document, and
    /// yrs declares it `Send + Sync`. The handle shares this cache, so a
    /// background encode also serves — and refreshes — what the UI thread would
    /// have computed.
    pub(crate) fn handle(&self, doc: &Doc) -> DocumentStateHandle {
        DocumentStateHandle {
            doc: doc.clone(),
            cache: Arc::clone(&self.inner),
        }
    }

    /// Return the document's full `state_v1`, re-encoding via `encode` only when the
    /// document changed since the last call.
    pub(crate) fn get(&self, encode: impl FnOnce() -> Vec<u8>) -> Vec<u8> {
        self.get_shared(encode).to_vec()
    }

    /// The same state, shared rather than copied.
    ///
    /// Handing out a copy costs the length of the document on every call, and
    /// the save path asks all of them for it: on a 178-document workspace that
    /// was 8 MB of memcpy on the UI thread every time a save came due. Callers
    /// that only pass the bytes along take the `Arc`; the ones that need an
    /// owned `Vec` (wire updates) still call [`Self::get`].
    pub(crate) fn get_shared(&self, encode: impl FnOnce() -> Vec<u8>) -> Arc<[u8]> {
        self.inner.get_shared(encode)
    }
}

impl EncodeCacheState {
    fn get_shared(&self, encode: impl FnOnce() -> Vec<u8>) -> Arc<[u8]> {
        // Clear dirty up front: a change racing in during `encode` re-sets it, so the
        // next call recomputes rather than serving a stale cache.
        if !self.dirty.swap(false, std::sync::atomic::Ordering::AcqRel) {
            if let Some(bytes) = self
                .bytes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                return Arc::clone(bytes);
            }
        }
        let bytes: Arc<[u8]> = Arc::from(encode());
        *self.bytes.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&bytes));
        bytes
    }
}

/// Produces one document's persisted state without needing the document's
/// owner, so the save path can encode on a background thread instead of on the
/// UI thread — where serializing a large scheme was an ~8 ms stall every time a
/// save came due.
///
/// The state it produces is read at the moment [`Self::encode`] runs, which may
/// be marginally later than the workspace snapshot it is saved alongside. The
/// two writes were never atomic with respect to each other anyway (a crash
/// between them already skews either way), and the skew this adds is a few
/// milliseconds of edits against a window that was already the whole write.
pub struct DocumentStateHandle {
    doc: Doc,
    cache: Arc<EncodeCacheState>,
}

impl DocumentStateHandle {
    /// The document's full state, re-encoding only if it changed since the last
    /// call — from either thread, since the cache is shared with its document.
    pub fn encode(&self) -> Arc<[u8]> {
        self.cache
            .get_shared(|| self.doc.transact().encode_diff_v1(&StateVector::default()))
    }
}

mod encoding;
mod scheme_content;
mod update_capture;
mod validation;
mod workspace_index;

use update_capture::{Delta, UpdateCapture};

pub use encoding::stable_client_id;
pub use scheme_content::YrsSchemeDocument;
pub use validation::validate_crdt_update_sequence;

pub(crate) use encoding::{
    decode_inline_embed_str, encode_inline_embed, random_document_client_id,
    serde_json_string_value, stable_item_seed_client_id, update_v1_is_empty, yrs_doc_options,
};
pub(crate) use scheme_content::item_text_ref;
#[cfg(test)]
pub(crate) use scheme_content::{item_map_ref, item_snapshot_json, write_new_item};
pub(crate) use validation::{validate_scheme_document, validate_workspace_document};
pub(crate) use workspace_index::{
    preserve_local_calendar_sync_token, scheme_documents_by_id, scheme_meta,
    workspace_document_snapshot, YrsJsonDocument,
};

const SCHEME_SCHEMA_V1: &str = "knotq.scheme_file.v1";
const WORKSPACE_SCHEMA_V1: &str = "knotq.workspace.v1";
const INLINE_EMBED_PREFIX: &str = "\u{fffc}knotq.inline.v1\0";

const NODE_KIND_FOLDER: &str = "folder";
const NODE_KIND_SCHEME: &str = "scheme";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceCrdtChangeSet {
    pub workspace: bool,
    pub schemes: HashSet<SchemeId>,
}

impl WorkspaceCrdtChangeSet {
    pub fn workspace(mut self) -> Self {
        self.workspace = true;
        self
    }

    pub fn touch_scheme(mut self, scheme: SchemeId) -> Self {
        self.schemes.insert(scheme);
        self
    }

    pub fn merge(&mut self, other: Self) {
        self.workspace |= other.workspace;
        self.schemes.extend(other.schemes);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceCrdtSyncOutcome {
    pub updates: Vec<CrdtDocumentUpdate>,
    pub errors: Vec<String>,
}

impl WorkspaceCrdtSyncOutcome {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn push_error(&mut self, context: impl std::fmt::Display, error: anyhow::Error) {
        self.errors.push(format!("{context}: {error:#}"));
    }
}

/// A recoverable per-document error during `apply_remote_updates`. Attributable
/// to one specific document; the caller can skip that document while still applying
/// the rest.
#[derive(Clone, Debug)]
pub struct DocumentApplyError {
    pub document: DocumentId,
    pub kind: knotq_model::SyncDocumentKind,
    /// True when the error is specifically "unknown scheme document" (the
    /// document ID arrived in a pull response but is not in the workspace index).
    /// This is a normal, benign situation: a scheme deleted on another device
    /// leaves its content doc on the server; callers should skip silently rather
    /// than alarm.
    pub unknown_scheme_document: bool,
    pub message: String,
}

/// A fatal workspace-document error during `apply_remote_updates`. The workspace
/// index itself is corrupt or inconsistent; the caller must not proceed with
/// applying scheme content and should abort the pull.
#[derive(Clone, Debug)]
pub struct WorkspaceApplyError {
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct WorkspaceCrdtApplyOutcome {
    pub workspace: Workspace,
    pub applied: usize,
    /// Per-document (scheme) errors: recoverable, attributable to one document.
    pub document_errors: Vec<DocumentApplyError>,
    /// Workspace-level fatal errors: if non-empty the caller must abort the pull.
    pub workspace_errors: Vec<WorkspaceApplyError>,
}

impl WorkspaceCrdtApplyOutcome {
    pub fn is_ok(&self) -> bool {
        self.document_errors.is_empty() && self.workspace_errors.is_empty()
    }

    pub fn workspace_is_ok(&self) -> bool {
        self.workspace_errors.is_empty()
    }

    fn push_workspace_error(&mut self, context: impl std::fmt::Display, error: anyhow::Error) {
        self.workspace_errors.push(WorkspaceApplyError {
            message: format!("{context}: {error:#}"),
        });
    }

    fn push_document_error(
        &mut self,
        document: DocumentId,
        kind: knotq_model::SyncDocumentKind,
        unknown_scheme_document: bool,
        context: impl std::fmt::Display,
        error: anyhow::Error,
    ) {
        self.document_errors.push(DocumentApplyError {
            document,
            kind,
            unknown_scheme_document,
            message: format!("{context}: {error:#}"),
        });
    }
}

pub struct WorkspaceCrdtDocuments {
    workspace: YrsJsonDocument,
    schemes: HashMap<SchemeId, YrsSchemeDocument>,
}

impl WorkspaceCrdtDocuments {
    pub fn snapshot_updates(workspace: &Workspace) -> WorkspaceCrdtSyncOutcome {
        let mut docs = Self::empty(workspace);
        docs.sync_changes(workspace, &WorkspaceCrdtChangeSet::default().workspace())
    }

    pub fn snapshot_updates_with_client_ids(
        workspace: &Workspace,
        workspace_client_id: u64,
        mut scheme_client_id: impl FnMut(SchemeId, DocumentId) -> u64,
    ) -> WorkspaceCrdtSyncOutcome {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        let mut docs = Self {
            workspace: YrsJsonDocument::new_with_client_id(
                workspace.sync.id,
                SyncDocumentKind::PersonalWorkspace,
                workspace_client_id,
            ),
            schemes: HashMap::new(),
        };
        docs.sync_changes_with_scheme_factory(
            &workspace,
            &WorkspaceCrdtChangeSet::default().workspace(),
            |scheme_id, document_id| {
                YrsSchemeDocument::new_with_client_id(
                    document_id,
                    scheme_client_id(scheme_id, document_id),
                )
            },
        )
    }

    pub fn empty(workspace: &Workspace) -> Self {
        Self::empty_inner(workspace, None)
    }

    /// Like [`empty`](Self::empty) but every document carries a stable, deterministic
    /// clientID for `replica_id` (see [`stable_client_id`]).
    pub fn empty_for_replica(workspace: &Workspace, replica_id: ReplicaId) -> Self {
        Self::empty_inner(workspace, Some(replica_id))
    }

    fn empty_inner(workspace: &Workspace, replica_id: Option<ReplicaId>) -> Self {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        Self {
            workspace: YrsJsonDocument::for_replica(
                workspace.sync.id,
                SyncDocumentKind::PersonalWorkspace,
                replica_id,
            ),
            schemes: HashMap::new(),
        }
    }

    pub fn try_new(workspace: &Workspace) -> anyhow::Result<Self> {
        let mut docs = Self::empty(workspace);
        docs.replace_all(workspace)?;
        Ok(docs)
    }

    /// Reconstruct the long-lived CRDT documents for `replica_id` from previously
    /// persisted per-document `state_v1` bytes. Documents present in `states` are
    /// restored exactly (preserving their Yjs identity and clocks). Documents absent
    /// from `states` are created EMPTY — never seeded from the materialized workspace.
    ///
    /// Seeding a fresh base for an absent document is what corrupts sync: a device
    /// that discovers another device's document (or a legacy server snapshot) would
    /// mint its own competing base under its clientID, and a later delta would
    /// tombstone the server's items while its replacements — built on that
    /// never-pushed local base — buffer unintegrated, wiping the document. Instead,
    /// an absent document is left empty here and populated either by the pull
    /// (adopting the server's canonical identity) or, for genuinely local content, by
    /// the store's reconcile, which force-emits a full snapshot establishing this
    /// device as the creator. This is the single way the real drivers obtain their
    /// CRDT: they never rebuild from plain data with a throwaway identity.
    /// Generic over the byte container so a caller can pass the shared states
    /// straight from [`Self::document_states`] without copying them first.
    pub fn from_states<B: AsRef<[u8]>>(
        workspace: &Workspace,
        // The replica id is no longer used for clientID derivation — every document is
        // built under a fresh random authoring identity (see below) — but the parameter
        // is kept so the desktop/mobile/test call sites stay unchanged.
        _replica_id: ReplicaId,
        states: &HashMap<DocumentId, B>,
    ) -> anyhow::Result<Self> {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        // Every document is (re)constructed under a FRESH random authoring clientID — the
        // standard Yjs session model — never the per-replica `stable_client_id`. Restored
        // bytes carry their own original authoring clientIDs, so this session's fresh id is
        // used ONLY for new local edits; existing content is never re-authored (the diff in
        // `replace_scheme`/`sync_snapshot` only writes changes). A stable clientID, by
        // contrast, gets REUSED across document incarnations, and when a clock ever
        // restarts (a rebuild whose restored bytes don't already contain that clientID's
        // full history) two unrelated operations alias the same `(clientID, clock)`. Yjs
        // then keeps whichever integrated first, so the merge becomes order-dependent — the
        // server (base-then-push) and the device (local-then-pull) land on different sides
        // and diverge forever (observed: a text chunk overwriting `scheme_file.id`, and a
        // delete that never takes on the server). A fresh random id per construction makes
        // `(clientID, clock)` reuse impossible, so every merge is commutative and converges
        // (worst case: duplicated content, which still converges identically everywhere).
        let workspace_state = states
            .get(&workspace.sync.id)
            .map(AsRef::as_ref)
            .filter(|state: &&[u8]| !state.is_empty());
        let workspace_doc = YrsJsonDocument::for_replica(
            workspace.sync.id,
            SyncDocumentKind::PersonalWorkspace,
            None,
        );
        if let Some(state) = workspace_state {
            workspace_doc
                .apply_update_v1(state)
                .context("restore workspace CRDT state")?;
        }
        let mut schemes = HashMap::new();
        // Sorted: each restored document takes a fresh random clientID, so the
        // iteration order decides which scheme gets which id. Content converges
        // either way, but a seeded property-fuzz run has to replay exactly, and
        // `from_states` runs on every sync.
        let mut ordered: Vec<SchemeId> = workspace.schemes.keys().copied().collect();
        ordered.sort();
        for id in &ordered {
            let meta = scheme_meta(&workspace, *id)?;
            let doc = YrsSchemeDocument::for_replica(meta.id, None);
            if let Some(state) = states
                .get(&meta.id)
                .map(AsRef::as_ref)
                .filter(|state: &&[u8]| !state.is_empty())
            {
                doc.apply_update_v1(state)
                    .with_context(|| format!("restore scheme CRDT state {id}"))?;
            }
            schemes.insert(*id, doc);
        }
        Ok(Self {
            workspace: workspace_doc,
            schemes,
        })
    }

    /// The set of document IDs for which this instance holds a local CRDT doc.
    /// Used by the engine to detect scheme documents that are now in the workspace
    /// index but have no local CRDT representation (so their cursor can be reset).
    pub fn known_document_ids(&self) -> std::collections::HashSet<DocumentId> {
        let mut ids = std::collections::HashSet::new();
        ids.insert(self.workspace.id);
        for doc in self.schemes.values() {
            ids.insert(doc.id);
        }
        ids
    }

    /// Snapshot every owned document's full `state_v1`, keyed by document id, for
    /// durable persistence. Restoring these via [`from_states`](Self::from_states)
    /// with the same `replica_id` round-trips the documents losslessly.
    /// Every owned document's authoring clientID. Exposed for the test that
    /// pins them to the document half of the partitioned clientID space.
    pub fn probe_client_ids(&self) -> Vec<u64> {
        let mut ids = vec![self.workspace.client_id()];
        ids.extend(self.schemes.values().map(|doc| doc.client_id()));
        ids
    }

    /// Every owned document's persisted state, shared rather than copied.
    ///
    /// The states are handed straight to the writer, so there is nothing to gain
    /// from each caller owning its own 8 MB of them — and this runs on the UI
    /// thread every time a save comes due.
    pub fn document_states(&self) -> HashMap<DocumentId, Arc<[u8]>> {
        let mut out = HashMap::new();
        out.insert(self.workspace.id, self.workspace.encode_state_shared_v1());
        for doc in self.schemes.values() {
            out.insert(doc.id, doc.encode_state_shared_v1());
        }
        out
    }

    /// The same states, but as handles that encode on demand — so the caller can
    /// do the encoding somewhere other than here.
    ///
    /// Collecting these is cheap whatever the workspace holds: each is a shared
    /// handle to a document and its cache, not the document's bytes. Encoding a
    /// large scheme is several milliseconds, and [`Self::document_states`] does
    /// it on whichever thread owns the documents — the UI thread, on every save.
    pub fn document_state_handles(&self) -> HashMap<DocumentId, DocumentStateHandle> {
        let mut out = HashMap::with_capacity(self.schemes.len() + 1);
        out.insert(self.workspace.id, self.workspace.state_handle());
        for doc in self.schemes.values() {
            out.insert(doc.id, doc.state_handle());
        }
        out
    }

    /// A full-state update for every owned document, taken from the live documents
    /// (so it carries their real clientID and clocks). Used to re-seed a server that
    /// has no base for a document, so the re-seed shares identity with the device's
    /// incremental diffs instead of competing with them under a throwaway identity.
    pub fn full_snapshot_updates(&self) -> WorkspaceCrdtSyncOutcome {
        let mut outcome = WorkspaceCrdtSyncOutcome::default();
        outcome.updates.push(CrdtDocumentUpdate {
            document: self.workspace.id,
            kind: self.workspace.kind,
            update_v1: self.workspace.encode_state_v1(),
            touched_items: Vec::new(),
        });
        // Emit schemes in a stable order. `self.schemes` is a HashMap, whose
        // iteration order is randomized per process; the caller queues these as
        // pending edits, so an unstable order gives edits nondeterministic
        // local-sequence numbers. Content still converges either way (CRDT merges
        // commute), but a deterministic order keeps a fuzz seed — and any
        // real-world "reproduce the exact push sequence" investigation —
        // reproducible run to run.
        let mut docs: Vec<&YrsSchemeDocument> = self.schemes.values().collect();
        docs.sort_by_key(|doc| doc.id);
        for doc in docs {
            // A full snapshot re-asserts every live item, so for the epoch
            // adoption rescue all of them count as locally touched (a queued
            // snapshot exists precisely to re-establish this device's content).
            let mut touched_items: Vec<String> = doc
                .scheme_items()
                .map(|items| items.iter().map(|item| item.id.to_string()).collect())
                .unwrap_or_default();
            touched_items.sort();
            outcome.updates.push(CrdtDocumentUpdate {
                document: doc.id,
                kind: SyncDocumentKind::Scheme,
                update_v1: doc.encode_state_v1(),
                touched_items,
            });
        }
        outcome
    }

    /// Re-label the personal-workspace CRDT document with `new_id`, preserving its
    /// current content and Yjs history (the real clientIDs and clocks). Returns the
    /// relabeled document's full-state update when the id actually changed — so the
    /// caller can queue it for push — or `None` if the document already had `new_id`.
    ///
    /// Used when a device adopts a different account's canonical workspace identity
    /// (a sign-in into an account this device did not last sync with — e.g. switching
    /// from prod to the sandbox). The two natural alternatives are both wrong here:
    /// keeping the old id makes every pull fail with a fatal document-id mismatch,
    /// and discarding the local document (rebuilding empty) loses every
    /// locally-created scheme because the workspace is materialized purely from the
    /// CRDT index. Instead we treat the local and server workspace documents as the
    /// same logical document and let the normal pull/push CRDT merge union their
    /// contents over the shared id. Unlike a throwaway re-seed, this carries the
    /// document's genuine history, so the merge integrates cleanly with the server's.
    pub fn reidentify_workspace_document(
        &mut self,
        new_id: DocumentId,
    ) -> anyhow::Result<Option<CrdtDocumentUpdate>> {
        if self.workspace.id == new_id {
            return Ok(None);
        }
        let kind = self.workspace.kind;
        let state = self.workspace.encode_state_v1();
        // Fresh random identity (not the stable per-replica clientID) — consistent with
        // `from_states`: the re-keyed doc carries the old content under its original
        // authoring clientIDs, and only new edits use this session's id, so no
        // `(clientID, clock)` is ever reused. See the rationale in `from_states`.
        let doc = YrsJsonDocument::for_replica(new_id, kind, None);
        doc.apply_update_v1(&state)
            .context("re-identify workspace CRDT document")?;
        self.workspace = doc;
        Ok(Some(CrdtDocumentUpdate {
            document: new_id,
            kind,
            update_v1: self.workspace.encode_state_v1(),
            touched_items: Vec::new(),
        }))
    }

    /// Rewrite any owned document whose current full state would fail the server's
    /// schema validation — i.e. an empty document with no schema root — by
    /// repopulating it from the materialized `workspace`. Such documents exist when
    /// a scheme is added to the workspace outside the command path (e.g. the
    /// desktop's direct Daily Queue creation): [`from_states`](Self::from_states)
    /// leaves it empty awaiting a pull, but if the server has no base for it either,
    /// its bootstrap snapshot is rejected as `crdt_schema_invalid` and wedges the
    /// whole push batch.
    ///
    /// `should_heal` gates which documents may be rewritten — callers restrict it to
    /// documents the server holds no base for (or has just rejected), so a heal
    /// never mints a base that competes with un-pulled server content. Returns the
    /// healed document ids.
    pub fn heal_schema_invalid_documents(
        &mut self,
        workspace: &Workspace,
        mut should_heal: impl FnMut(DocumentId) -> bool,
    ) -> Vec<DocumentId> {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        let mut healed = Vec::new();
        let state_is_invalid = |kind: SyncDocumentKind, state: Vec<u8>| {
            validate_crdt_update_sequence(kind, [state.as_slice()]).is_err()
        };
        if should_heal(self.workspace.id)
            && state_is_invalid(self.workspace.kind, self.workspace.encode_state_v1())
        {
            match self
                .workspace
                .sync_snapshot(&workspace_document_snapshot(&workspace), true)
            {
                Ok(_) => healed.push(self.workspace.id),
                Err(err) => eprintln!("heal workspace CRDT document failed: {err:#}"),
            }
        }
        // Sorted: `healed` is order-bearing downstream (it gates which pending
        // edits get replaced by a snapshot).
        let mut heal_ids: Vec<SchemeId> = self.schemes.keys().copied().collect();
        heal_ids.sort();
        for scheme_id in &heal_ids {
            let Some(doc) = self.schemes.get(scheme_id) else {
                continue;
            };
            if !should_heal(doc.id)
                || !state_is_invalid(SyncDocumentKind::Scheme, doc.encode_state_v1())
            {
                continue;
            }
            let Some(scheme) = workspace.schemes.get(scheme_id) else {
                continue;
            };
            match doc.replace_scheme(scheme) {
                Ok(_) => healed.push(doc.id),
                Err(err) => eprintln!("heal scheme CRDT document {scheme_id} failed: {err:#}"),
            }
        }
        healed
    }

    /// Adopt a squashed (epoch-bumped) scheme document: REPLACE the local CRDT
    /// document with `state` instead of merging (the squashed document shares no
    /// Yjs history with its predecessor, so a merge would double content), then
    /// re-express any un-pushed local edits against it.
    ///
    /// `pending_touched` communicates the local pending edits for this document:
    /// `None` means there are none (the common case — the document is replaced
    /// wholesale and the result is exact). `Some(touched)` triggers an
    /// item-granular three-way rescue between the local materialized scheme
    /// (which includes the pending edits) and the adopted remote content: items
    /// in `touched` keep their local version (including local deletions), all
    /// other items take the remote version (including remote deletions and
    /// post-squash remote edits). The rescue is returned as a fresh update
    /// authored against the adopted document, for the caller to queue as
    /// new-epoch pending.
    ///
    /// Returns the re-materialized workspace alongside the optional rescue.
    pub fn adopt_squashed_document(
        &mut self,
        current: &Workspace,
        document: DocumentId,
        state: &[u8],
        pending_touched: Option<&HashSet<String>>,
    ) -> anyhow::Result<(Workspace, Option<CrdtDocumentUpdate>)> {
        let scheme_id = scheme_documents_by_id(current)
            .get(&document)
            .copied()
            .ok_or_else(|| anyhow!("unknown scheme document {document}"))?;
        let adopted = YrsSchemeDocument::for_replica(document, None);
        adopted
            .apply_update_v1(state)
            .context("adopt squashed scheme state")?;
        adopted
            .validate()
            .context("validate squashed scheme state")?;

        let rescue = match (pending_touched, current.schemes.get(&scheme_id)) {
            (Some(touched), Some(local_scheme)) => {
                let remote_items = adopted.scheme_items()?;
                let merged = merge_items_for_adoption(&local_scheme.items, remote_items, touched);
                let mut scheme = local_scheme.clone();
                scheme.items = merged;
                // Only a rescue that actually changes the adopted document is
                // queued; identical content diffs to an empty update -> None.
                adopted.sync_scheme(&scheme)?
            }
            _ => None,
        };

        self.schemes.insert(scheme_id, adopted);
        let workspace = self
            .materialize_workspace(current)
            .context("materialize after epoch adoption")?;
        Ok((workspace, rescue))
    }

    /// Scheme documents large enough to be worth squashing, as
    /// `(document, state_v1_len)`, largest first. The caller applies its own
    /// eligibility rules (fully synced, no pending) before calling
    /// [`rebuild_scheme_state`](Self::rebuild_scheme_state) on a candidate.
    pub fn squash_candidates(&self, min_state_bytes: usize) -> Vec<(DocumentId, usize)> {
        let mut candidates: Vec<(DocumentId, usize)> = self
            .schemes
            .values()
            .map(|doc| (doc.id, doc.encode_state_v1().len()))
            .filter(|(_, len)| *len >= min_state_bytes)
            .collect();
        candidates.sort_by(|left, right| right.1.cmp(&left.1));
        candidates
    }

    /// Rebuild `document`'s content as a fresh CRDT with no edit history — the
    /// state a squash proposes as the replacement. Built from the LIVE local
    /// document's materialized items (not the possibly-stale `workspace`
    /// snapshot) so the rebuild is exactly content-equivalent to what the
    /// server holds when this replica is fully synced.
    pub fn rebuild_scheme_state(&self, document: DocumentId) -> anyhow::Result<Vec<u8>> {
        let (scheme_id, doc) = self
            .schemes
            .iter()
            .find(|(_, doc)| doc.id == document)
            .ok_or_else(|| anyhow!("unknown scheme document {document}"))?;
        let items = doc.scheme_items()?;
        let scheme = Scheme {
            id: *scheme_id,
            // Only `id` and `items` land in the content document; the remaining
            // fields live in the workspace index.
            name: String::new(),
            color_index: 0,
            gsync: false,
            source: SchemeSource::default(),
            items,
        };
        let rebuilt = YrsSchemeDocument::from_scheme(document, &scheme)?;
        Ok(rebuilt.encode_state_v1())
    }

    pub fn replace_all(&mut self, workspace: &Workspace) -> anyhow::Result<()> {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        self.workspace
            .replace_snapshot(&workspace_document_snapshot(&workspace))?;

        self.schemes
            .retain(|id, _| workspace.schemes.contains_key(id));
        // Sorted: a document created here takes a fresh random clientID, so
        // HashMap iteration order would decide which scheme receives which id.
        // Content converges either way, but a seeded run must replay exactly.
        let mut ordered: Vec<SchemeId> = workspace.schemes.keys().copied().collect();
        ordered.sort();
        for id in &ordered {
            let scheme = &workspace.schemes[id];
            let meta = scheme_meta(&workspace, *id)?;
            // A doc created here starts from an empty base (no restored bytes), so it
            // gets a fresh identity — never the stable clientID, which is reserved for
            // from-bytes restore (see `from_states`) to avoid `(clientID, clock)` reuse.
            self.schemes
                .entry(*id)
                .or_insert_with(|| YrsSchemeDocument::for_replica(meta.id, None))
                .replace_scheme(scheme)
                .with_context(|| format!("replace scheme CRDT {id}"))?;
        }
        Ok(())
    }

    pub fn sync_changes(
        &mut self,
        workspace: &Workspace,
        changeset: &WorkspaceCrdtChangeSet,
    ) -> WorkspaceCrdtSyncOutcome {
        // A doc absent from `self.schemes` is authored from an empty base, so it gets a
        // fresh identity (`None`); the stable clientID is reserved for from-bytes restore
        // in `from_states` to prevent `(clientID, clock)` reuse across incarnations.
        self.sync_changes_with_scheme_factory(workspace, changeset, move |_, document_id| {
            YrsSchemeDocument::for_replica(document_id, None)
        })
    }

    fn sync_changes_with_scheme_factory(
        &mut self,
        workspace: &Workspace,
        changeset: &WorkspaceCrdtChangeSet,
        mut new_scheme_document: impl FnMut(SchemeId, DocumentId) -> YrsSchemeDocument,
    ) -> WorkspaceCrdtSyncOutcome {
        // Only clone to repair. This runs on the per-keystroke path, where the
        // workspace virtually always already satisfies the invariants — and a
        // clone of it is the single most expensive thing an ordinary edit did
        // (~0.7 ms on a 177-scheme workspace, against ~0.3 ms for the actual
        // document write).
        let workspace = if workspace.sync_metadata_is_current() {
            Cow::Borrowed(workspace)
        } else {
            let mut repaired = workspace.clone();
            repaired.ensure_sync_metadata();
            Cow::Owned(repaired)
        };
        let workspace = workspace.as_ref();
        let mut outcome = WorkspaceCrdtSyncOutcome::default();

        let workspace_documents_missing = documents_missing(self, workspace);
        let workspace_documents_removed = documents_removed(self, workspace);
        if changeset.workspace || workspace_documents_missing || workspace_documents_removed {
            // A document-set change (a scheme added or removed) must re-emit the
            // full workspace state so a server that lost the document can rebuild
            // it; an ordinary edit emits only the incremental diff.
            let force = workspace_documents_missing || workspace_documents_removed;
            match self
                .workspace
                .sync_snapshot(&workspace_document_snapshot(workspace), force)
            {
                Ok(Some(update)) => outcome.updates.push(update),
                Ok(None) => {}
                Err(err) => outcome.push_error("workspace CRDT update", err),
            }
        }

        let mut scheme_ids: HashSet<SchemeId> = changeset.schemes.iter().copied().collect();
        scheme_ids.extend(
            workspace
                .schemes
                .keys()
                .copied()
                .filter(|id| !self.schemes.contains_key(id)),
        );
        self.schemes
            .retain(|id, _| workspace.schemes.contains_key(id));
        // Sorted for the same reason as `replace_all`: a first-sight document
        // takes a fresh random clientID here, and the emitted updates are queued
        // in this order, so HashMap order would make a seeded run unreplayable.
        let mut scheme_ids: Vec<SchemeId> = scheme_ids.into_iter().collect();
        scheme_ids.sort();
        for id in scheme_ids {
            let Some(scheme) = workspace.schemes.get(&id) else {
                continue;
            };
            let meta = match scheme_meta(workspace, id) {
                Ok(meta) => meta,
                Err(err) => {
                    outcome.push_error(format!("scheme CRDT metadata {id}"), err);
                    continue;
                }
            };
            match self
                .schemes
                .entry(id)
                .or_insert_with(|| new_scheme_document(id, meta.id))
                .sync_scheme(scheme)
            {
                Ok(Some(update)) => outcome.updates.push(update),
                Ok(None) => {}
                Err(err) => outcome.push_error(format!("scheme CRDT update {id}"), err),
            }
        }

        outcome
    }

    pub fn apply_remote_updates(
        &mut self,
        current: &Workspace,
        updates: &[StoredCrdtUpdate],
    ) -> WorkspaceCrdtApplyOutcome {
        let mut outcome = WorkspaceCrdtApplyOutcome {
            workspace: current.clone(),
            applied: 0,
            document_errors: Vec::new(),
            workspace_errors: Vec::new(),
        };

        let mut workspace_applied = false;
        let mut workspace_update_eligible_for_materialization = false;
        for update in updates
            .iter()
            .filter(|update| update.kind == SyncDocumentKind::PersonalWorkspace)
        {
            if update.document != self.workspace.id {
                outcome.push_workspace_error(
                    format!("workspace update {}", update.sequence),
                    anyhow!(
                        "document id mismatch: expected {}, got {}",
                        self.workspace.id,
                        update.document
                    ),
                );
                continue;
            }
            workspace_update_eligible_for_materialization = true;
            match self.workspace.apply_update_v1(&update.update_v1) {
                // Only a merge that actually changed the document counts as
                // applied. An echo of this replica's own push (the server
                // broadcasts `changed` to every device, including the origin)
                // merges as a no-op and must not trigger re-materialization —
                // otherwise every local edit bounces back as a phantom "remote
                // change" that rebuilds views and clobbers in-progress editing.
                Ok(true) => {
                    outcome.applied += 1;
                    workspace_applied = true;
                }
                Ok(false) => {}
                Err(err) => outcome
                    .push_workspace_error(format!("workspace update {}", update.sequence), err),
            }
        }

        // Defense in depth: the client does not blindly trust remote bytes. After
        // applying remote updates, re-run the same schema validation the server
        // performs before materializing/persisting anything.
        if workspace_update_eligible_for_materialization {
            if let Err(err) = self.workspace.validate() {
                outcome.push_workspace_error("workspace validation", err);
                return outcome;
            }
        }

        // No scheme content has been applied yet, so reuse `current`'s scheme items
        // wholesale; this first pass only reflects the workspace-structure document.
        // When no workspace update changed the doc (empty batch or pure echo),
        // `outcome.workspace` stays the `current` clone — re-materializing would
        // only re-derive the same content.
        if workspace_update_eligible_for_materialization {
            match self.materialize_workspace(current) {
                Ok(workspace) => {
                    // A no-op CRDT merge can still expose stale optimistic UI state:
                    // the long-lived document may already contain the server's
                    // resolution while `current` still holds a local color/name/
                    // folder/archive value. Only replace on a real workspace-index
                    // mismatch so a true own-push echo remains entirely inert.
                    if workspace_applied
                        || workspace_document_snapshot(&workspace)
                            != workspace_document_snapshot(current)
                    {
                        outcome.workspace = workspace;
                    }
                }
                Err(err) => {
                    outcome.push_workspace_error("workspace materialization", err);
                    return outcome;
                }
            }
        }

        // An unseeded workspace document cannot describe anything, so nothing
        // below it can be materialized. Refusing here — after the workspace pass,
        // which is what seeds it on a device's very first pull — costs a fresh
        // device nothing and stops a broken one from merging remote content into
        // its scheme documents only to fail at the end with "workspace id
        // missing", which is where this used to surface.
        //
        // In practice the cause is a data directory written by a NEWER build:
        // the per-document CRDT-state layout is invisible to a build that
        // predates it, so that build loads zero documents and starts from an
        // empty workspace document while the real state sits on disk beside it.
        if !self.workspace.is_seeded() {
            outcome.push_workspace_error(
                "local CRDT state is empty",
                anyhow!(
                    "the workspace document holds nothing while {} schemes exist \
                     locally; the data directory was most likely written by a newer \
                     build of KnotQ. No remote updates were applied.",
                    current.schemes.len()
                ),
            );
            return outcome;
        }

        self.schemes
            .retain(|id, _| outcome.workspace.schemes.contains_key(id));
        let scheme_by_document = scheme_documents_by_id(&outcome.workspace);
        // Track which scheme documents had errors so their cursor can be reset later.
        let mut touched_schemes: HashSet<SchemeId> = HashSet::new();
        // Track schemes that had a per-document error (to exclude from validation).
        let mut errored_schemes: HashSet<SchemeId> = HashSet::new();
        for update in updates
            .iter()
            .filter(|update| update.kind == SyncDocumentKind::Scheme)
        {
            let Some(scheme_id) = scheme_by_document.get(&update.document).copied() else {
                // The content document arrived but its scheme is not in the workspace
                // index. This is a normal occurrence: a scheme deleted on one device
                // leaves its content doc on the server, or an orphan was created by a
                // buggy heal path. We skip silently; the cursor will be advanced so we
                // do not re-pull this every cycle.
                outcome.push_document_error(
                    update.document,
                    SyncDocumentKind::Scheme,
                    true, // unknown_scheme_document
                    format!("scheme update {}", update.sequence),
                    anyhow!("unknown scheme document {}", update.document),
                );
                continue;
            };
            // First sight of this content doc: create it from an empty base and adopt
            // the server's structs from the update below. A fresh identity (`None`) — not
            // the stable clientID — keeps it from reusing a `(clientID, clock)` the server
            // may already hold under that clientID from a prior local incarnation.
            match self
                .schemes
                .entry(scheme_id)
                .or_insert_with(|| YrsSchemeDocument::for_replica(update.document, None))
                .apply_update_v1(&update.update_v1)
            {
                // As with the workspace document above: an echoed no-op merge
                // must not mark the scheme touched, or the scheme the user is
                // actively editing gets re-materialized (and the UI reloaded)
                // on every round-trip of their own keystrokes.
                Ok(true) => {
                    outcome.applied += 1;
                    touched_schemes.insert(scheme_id);
                }
                Ok(false) => {
                    // A byte-level no-op is usually an echo of this replica's own
                    // push. It can also be the first update pulled after concurrent
                    // text edits resolved differently from the optimistic plaintext
                    // still held by the UI. In that case the CRDT already contains
                    // the server result, so applying the update changes no structs,
                    // but skipping materialization would leave the visible workspace
                    // permanently stale. Compare before opting out: true echoes stay
                    // cheap and do not disturb active editing.
                    let visible_items = outcome
                        .workspace
                        .schemes
                        .get(&scheme_id)
                        .map(|scheme| &scheme.items);
                    let crdt_items = self
                        .schemes
                        .get(&scheme_id)
                        .and_then(|document| document.scheme_items().ok());
                    if matches!(
                        (visible_items, crdt_items.as_ref()),
                        (Some(visible), Some(authoritative))
                            if visible.as_slice() != authoritative.as_slice()
                    ) {
                        touched_schemes.insert(scheme_id);
                    }
                }
                Err(err) => {
                    let doc_id = update.document;
                    outcome.push_document_error(
                        doc_id,
                        SyncDocumentKind::Scheme,
                        false,
                        format!("scheme update {}", update.sequence),
                        err,
                    );
                    errored_schemes.insert(scheme_id);
                }
            }
        }

        for scheme_id in &touched_schemes {
            if errored_schemes.contains(scheme_id) {
                continue; // already recorded an error for this scheme
            }
            if let Some(doc) = self.schemes.get(scheme_id) {
                if let Err(err) = doc.validate() {
                    // Attribute the validation failure to this scheme's document id.
                    let doc_id = outcome
                        .workspace
                        .scheme_sync
                        .get(scheme_id)
                        .map(|m| m.id)
                        .unwrap_or_default();
                    outcome.push_document_error(
                        doc_id,
                        SyncDocumentKind::Scheme,
                        false,
                        format!("scheme validation {scheme_id}"),
                        err,
                    );
                    errored_schemes.insert(*scheme_id);
                }
            }
        }

        if !touched_schemes.is_empty() {
            match self.materialize_workspace(current) {
                Ok(workspace) => outcome.workspace = workspace,
                Err(err) => outcome.push_workspace_error("scheme materialization", err),
            }
        }

        outcome
    }

    /// [`Self::materialize_workspace`] for diagnostics: rebuilding the workspace
    /// straight from the CRDT and comparing it against what is on disk is how you
    /// check a data directory is intact, and that is worth being able to do from
    /// outside this crate.
    pub fn materialized_workspace_for_diagnostics(
        &self,
        current: &Workspace,
    ) -> anyhow::Result<Workspace> {
        self.materialize_workspace(current)
    }

    /// Rebuild the workspace from the CRDT documents, using `current` only for
    /// state the documents do not carry (a scheme with no local document, and
    /// the local-only calendar sync token).
    fn materialize_workspace(&self, current: &Workspace) -> anyhow::Result<Workspace> {
        let snapshot: WorkspaceDocumentSnapshot = self.workspace.snapshot()?;
        let scheme_sync = snapshot
            .scheme_sync
            .into_iter()
            .map(|entry| (entry.scheme, entry.sync))
            .collect::<HashMap<_, _>>();
        let folder_sync = snapshot
            .folder_sync
            .into_iter()
            .map(|entry| (entry.folder, entry.sync))
            .collect::<HashMap<_, _>>();
        let mut workspace = Workspace {
            id: snapshot.id,
            sync: snapshot.sync,
            root: snapshot.root,
            folders: snapshot
                .folders
                .into_iter()
                .map(|folder| (folder.id, folder))
                .collect(),
            schemes: HashMap::new(),
            scheme_sync,
            folder_sync,
            daily_queue: snapshot
                .daily_queue
                .into_iter()
                .map(|entry| (entry.date, entry.scheme))
                .collect(),
            recently_deleted: snapshot.recently_deleted,
            deleted_scheme_origins: snapshot
                .deleted_scheme_origins
                .into_iter()
                .map(|entry| (entry.scheme, entry.origin))
                .collect(),
            recently_deleted_folders: snapshot.recently_deleted_folders,
            deleted_folder_origins: snapshot
                .deleted_folder_origins
                .into_iter()
                .map(|entry| (entry.folder, entry.origin))
                .collect(),
        };

        for entry in snapshot.schemes {
            // The document is authoritative: derive items from it whenever this
            // replica has one, and fall back to `current` only when it does not.
            //
            // This used to reuse `current`'s items for any scheme not in
            // `changed_schemes`, on the assumption that such a document is
            // byte-identical to what produced `current`. That assumption does not
            // hold. A document can take remote content in a merge whose result is
            // never adopted — an aborted pull, a rejected batch — and because Yjs
            // replays an already-delivered update as a no-op, the scheme is never
            // marked changed again. The reuse then pinned the stale items
            // *permanently*: the device's document and the server agreed while its
            // workspace showed older content, forever (found by the property
            // fuzzer at 3000 seeds, `undo_redo_fuzz_converges`).
            //
            // Deriving unconditionally costs ~16 ms on a 168-scheme / 3.7k-item
            // workspace, and only on a pull that changed something — a background
            // sync step, not the interactive path.
            let items = self
                .schemes
                .get(&entry.id)
                .and_then(|doc| doc.scheme_items().ok())
                .or_else(|| {
                    current
                        .schemes
                        .get(&entry.id)
                        .map(|scheme| scheme.items.clone())
                })
                .unwrap_or_default();
            workspace.schemes.insert(
                entry.id,
                Scheme {
                    id: entry.id,
                    name: entry.name,
                    color_index: entry.color_index,
                    gsync: entry.gsync,
                    source: preserve_local_calendar_sync_token(current, entry.id, entry.source),
                    items,
                },
            );
        }

        workspace.ensure_sync_metadata();
        Ok(workspace)
    }
}

/// Item-granular three-way merge for epoch adoption, with the local pending
/// edits' `touched` set standing in for the missing common base:
///   - an item the local pending edits touched keeps its LOCAL fate — the local
///     version if present, dropped if locally deleted;
///   - every other item takes its REMOTE fate — the remote version if present
///     (covering post-squash remote edits), dropped if absent remotely (a
///     remote deletion, or an item this replica never pushed... which cannot
///     exist untouched, since unpushed local additions are always touched).
///
/// Ordering follows the remote list; rescued local-only items are inserted
/// after their nearest preceding local neighbour that survived the merge.
pub(crate) fn merge_items_for_adoption(
    local: &[Item],
    remote: Vec<Item>,
    touched: &HashSet<String>,
) -> Vec<Item> {
    let local_by_id: HashMap<String, &Item> = local
        .iter()
        .map(|item| (item.id.to_string(), item))
        .collect();
    let remote_ids: HashSet<String> = remote.iter().map(|item| item.id.to_string()).collect();

    let mut merged: Vec<Item> = Vec::with_capacity(remote.len());
    for item in remote {
        let id = item.id.to_string();
        if touched.contains(&id) {
            if let Some(local_item) = local_by_id.get(&id) {
                merged.push((*local_item).clone());
            }
            // Touched but locally absent: a local deletion — honor it.
        } else {
            merged.push(item);
        }
    }

    // Rescue touched local items the remote does not have (local additions, or
    // local edits racing a remote deletion — conflict resolved toward keeping
    // content). Walk the local order so each lands after its local predecessor.
    for (index, item) in local.iter().enumerate() {
        let id = item.id.to_string();
        if remote_ids.contains(&id) || !touched.contains(&id) {
            continue;
        }
        let anchor = local[..index].iter().rev().find_map(|previous| {
            let previous_id = previous.id;
            merged.iter().position(|entry| entry.id == previous_id)
        });
        let at = anchor.map(|position| position + 1).unwrap_or(0);
        merged.insert(at, item.clone());
    }

    merged
}

fn documents_missing(docs: &WorkspaceCrdtDocuments, workspace: &Workspace) -> bool {
    workspace
        .schemes
        .keys()
        .any(|id| !docs.schemes.contains_key(id))
}

fn documents_removed(docs: &WorkspaceCrdtDocuments, workspace: &Workspace) -> bool {
    docs.schemes
        .keys()
        .any(|id| !workspace.schemes.contains_key(id))
}

/// One folder or scheme stored as an individual, id-keyed entry in the workspace
/// document's `nodes` map. `parent`/`position` carry the tree structure so that
/// it can be reconstructed (and merged) without a shared, wedge-prone array.
#[derive(Serialize, Deserialize)]
struct WorkspaceNodeEntry {
    id: String,
    kind: String,
    #[serde(default)]
    parent: String,
    #[serde(default)]
    position: String,
    payload: String,
}

#[derive(Serialize, Deserialize)]
struct FolderPayload {
    name: String,
    expanded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<FolderId>,
}

#[derive(Deserialize, PartialEq, Serialize)]
pub(crate) struct WorkspaceDocumentSnapshot {
    schema: String,
    id: knotq_model::WorkspaceId,
    sync: SyncDocumentMeta,
    root: FolderId,
    folders: Vec<Folder>,
    schemes: Vec<SchemeWorkspaceEntry>,
    daily_queue: Vec<DailyQueueEntry>,
    recently_deleted: Vec<SchemeId>,
    deleted_scheme_origins: Vec<DeletedSchemeOriginEntry>,
    recently_deleted_folders: Vec<FolderId>,
    deleted_folder_origins: Vec<DeletedFolderOriginEntry>,
    scheme_sync: Vec<SchemeSyncEntry>,
    folder_sync: Vec<FolderSyncEntry>,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct SchemeWorkspaceEntry {
    id: SchemeId,
    name: String,
    color_index: u8,
    gsync: bool,
    source: SchemeSource,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct DailyQueueEntry {
    date: NaiveDate,
    scheme: SchemeId,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct DeletedSchemeOriginEntry {
    scheme: SchemeId,
    origin: DeletedSchemeOrigin,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct DeletedFolderOriginEntry {
    folder: FolderId,
    origin: DeletedFolderOrigin,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct SchemeSyncEntry {
    scheme: SchemeId,
    sync: SyncDocumentMeta,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct FolderSyncEntry {
    folder: FolderId,
    sync: SyncDocumentMeta,
}

#[cfg(test)]
mod tests;
