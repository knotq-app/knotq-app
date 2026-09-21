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
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use chrono::{DateTime, NaiveDate};
use knotq_model::{
    DeletedFolderOrigin, DeletedSchemeOrigin, DocumentId, Folder, FolderId, Inline, Item,
    ItemContent, ItemId, ItemMarker, NodeRef, ReplicaId, Scheme, SchemeId, SchemeSource,
    SyncDocumentKind, SyncDocumentMeta, Workspace, PERMANENT_DELETE_TOMBSTONE_POSITION,
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
/// so the version below moves exactly when the serialized state would differ. A
/// keyed observer needs no retained `Subscription` (it lives and dies with the
/// document), keeping the document wrapper trivially constructible.
///
/// # Why a version and not a dirty flag
///
/// The cache is shared with every [`DocumentStateHandle`] taken from the
/// document, and the save task encodes those handles on a background thread. So
/// two threads can be inside [`EncodeCacheState::get_shared`] at once, and a
/// `dirty` flag cleared on the way *in* is wrong in both directions:
///
///  - whoever clears it CONSUMES the notice of an edit that has not been encoded
///    yet, so a concurrent reader finds a clean flag and is handed the bytes from
///    before that edit; and
///  - the slower of two encodes publishes last, overwriting a newer state with an
///    older one while the flag says the cache is current.
///
/// Both hand out a document state that is missing a local edit — and the desktop
/// rebuilds its live CRDT documents from exactly these bytes
/// (`WorkspaceStore::replace_workspace`), so the edit is not merely absent from
/// one snapshot: it is dropped from the document, and the next materialization
/// puts the user's deleted line back on screen.
///
/// Stamping the cached bytes with the version they were encoded at fixes both.
/// A reader serves the cache only when the stamp still matches the live version,
/// and an encode publishes only over an older stamp. The stamp is read *before*
/// encoding, so an edit that lands mid-encode makes the result look older than it
/// might be — which costs a re-encode and never a stale answer.
pub(crate) struct EncodeCache {
    inner: Arc<EncodeCacheState>,
}

/// The cache's mutable half, held behind an `Arc` so a [`DocumentStateHandle`]
/// can share it with a background thread. Without that, encoding a document's
/// state could only happen wherever the document lives — the UI thread.
#[derive(Default)]
struct EncodeCacheState {
    /// Bumped by the document's update observer. Monotonic, so a cached stamp
    /// can be compared against it without any lock.
    version: AtomicU64,
    /// The last published state and the version it was encoded at.
    cached: Mutex<Option<(u64, Arc<[u8]>)>>,
}

impl EncodeCache {
    /// Install the change-tracking observer on `doc` and return an empty cache.
    pub(crate) fn new(doc: &Doc) -> Self {
        let inner = Arc::new(EncodeCacheState::default());
        let versions = Arc::clone(&inner);
        let _ = doc.observe_update_v1_with("knotq_encode_cache", move |_txn, _evt| {
            versions.mark_dirty();
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
    /// The document changed: every cached state is now from an older version.
    fn mark_dirty(&self) {
        self.version.fetch_add(1, Ordering::Release);
    }

    fn get_shared(&self, encode: impl FnOnce() -> Vec<u8>) -> Arc<[u8]> {
        // Read the version BEFORE encoding. A change landing during `encode` moves
        // it past this stamp, so the result is published (and later read) as the
        // older state it might be, rather than being trusted as current.
        let version = self.version.load(Ordering::Acquire);
        if let Some((cached_version, bytes)) = self
            .cached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            if *cached_version == version {
                return Arc::clone(bytes);
            }
        }
        let bytes: Arc<[u8]> = Arc::from(encode());
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        // Publish only over an older state: a slow encode that finishes after a
        // newer one must not put the document back.
        if cached.as_ref().is_none_or(|(cached, _)| *cached < version) {
            *cached = Some((version, Arc::clone(&bytes)));
        }
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
/// Cloning shares the same document and cache, so clones agree with each other
/// and with the document they came from.
#[derive(Clone)]
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
    /// Item ids explicitly deleted by the local command batch, keyed by their
    /// source scheme. A scheme edit must preserve raw CRDT copies that are
    /// hidden by workspace-wide duplicate placement; only an explicit delete
    /// is evidence that such a copy should be tombstoned.
    pub deleted_items: HashMap<SchemeId, HashSet<String>>,
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
        for (scheme, items) in other.deleted_items {
            self.deleted_items.entry(scheme).or_default().extend(items);
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.workspace && self.schemes.is_empty() && self.deleted_items.is_empty()
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
    /// Document ids whose Yjs state changed (including a no-op merge that
    /// exposed stale materialized items). Drivers use this to scope durable
    /// persistence after a batched pull.
    pub changed_documents: HashSet<DocumentId>,
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
    /// Persisted `state_v1` bytes for scheme documents that are bound in the
    /// workspace index but not currently decoded into a live Yjs document.
    ///
    /// Mobile deliberately lazy-loads Daily Queue schemes outside the viewed
    /// date range: their scheme files are left unparsed and their entries stay
    /// out of the UI [`Workspace`]. Their sync bindings and their durable CRDT
    /// bytes are still on disk, though. Decoding every historical daily into a
    /// Yjs `Doc` at cold open (or re-deriving one on every caught-up pull) costs
    /// time proportional to the user's entire history for no visible benefit, so
    /// those bytes are held here verbatim instead.
    ///
    /// A deferred document is promoted into `schemes` (see
    /// [`Self::hydrate_deferred`]) only when it is
    /// - about to be edited or re-synced locally,
    /// - the target of an incoming remote update, or
    /// - explicitly requested for parser recovery.
    ///   It is never dropped by a save and never pruned by activity on an
    ///   unrelated scheme, and its bytes round-trip byte-for-byte through
    ///   [`Self::document_states`], so no document is ever lost by deferring it.
    deferred: HashMap<SchemeId, DeferredSchemeDocument>,
}

/// One entry of [`WorkspaceCrdtDocuments::deferred`]: the durable bytes of a
/// scheme document we own but have not decoded.
#[derive(Clone)]
struct DeferredSchemeDocument {
    document: DocumentId,
    /// A full `state_v1` snapshot (`encode_diff_v1` against the empty state
    /// vector), exactly as it was persisted — safe to re-emit as a bootstrap
    /// update or re-persist without any decode/re-encode round trip.
    state_v1: Arc<[u8]>,
}

/// Decode a deferred entry's persisted bytes into a live scheme document.
/// A fresh random authoring identity, consistent with [`from_states`] — the
/// restored bytes keep their own authoring clientIDs, so nothing is re-authored.
fn deferred_live_document(deferred: &DeferredSchemeDocument) -> anyhow::Result<YrsSchemeDocument> {
    let doc = YrsSchemeDocument::for_replica(deferred.document, None);
    doc.apply_update_v1(&deferred.state_v1)
        .with_context(|| format!("hydrate deferred scheme document {}", deferred.document))?;
    Ok(doc)
}

/// A count of how many scheme documents are decoded into live Yjs documents vs
/// held only as deferred bytes. Returned by
/// [`WorkspaceCrdtDocuments::document_population`] for structural assertions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CrdtDocumentPopulation {
    pub live_schemes: usize,
    pub deferred_schemes: usize,
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
            deferred: HashMap::new(),
        };
        docs.sync_changes_with_scheme_factory(
            &workspace,
            &WorkspaceCrdtChangeSet::default().workspace(),
            &HashMap::new(),
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
            deferred: HashMap::new(),
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
        replica_id: ReplicaId,
        states: &HashMap<DocumentId, B>,
    ) -> anyhow::Result<Self> {
        Self::from_states_with_options(workspace, replica_id, states, false)
    }

    /// Restore persisted documents without decoding every scheme at mobile cold
    /// start. Materialized scheme files remain available in `workspace`; their
    /// CRDT bytes stay in `deferred` until the scheme is edited, receives a
    /// remote update, or is included in an explicit integrity proof.
    ///
    /// This is deliberately separate from [`Self::from_states`]. Desktop and the
    /// property model retain eager restoration, while mobile can keep startup
    /// proportional to the documents it actually needs to display.
    pub fn from_states_lazy<B: AsRef<[u8]>>(
        workspace: &Workspace,
        replica_id: ReplicaId,
        states: &HashMap<DocumentId, B>,
    ) -> anyhow::Result<Self> {
        Self::from_states_with_options(workspace, replica_id, states, true)
    }

    fn from_states_with_options<B: AsRef<[u8]>>(
        workspace: &Workspace,
        // The replica id is no longer used for clientID derivation — every document is
        // built under a fresh random authoring identity (see below) — but the parameter
        // is kept so the desktop/mobile/test call sites stay unchanged.
        _replica_id: ReplicaId,
        states: &HashMap<DocumentId, B>,
        defer_materialized_schemes: bool,
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
        let mut deferred = HashMap::new();
        // Sorted: each restored document takes a fresh random clientID, so the
        // iteration order decides which scheme gets which id. Content converges
        // either way, but a seeded property-fuzz run has to replay exactly, and
        // `from_states` runs on every sync.
        //
        // Iterate the durable `scheme_sync` bindings, not the visible
        // `workspace.schemes`. `WorkspaceLoadOptions` deliberately omits
        // old/future Daily Queue schemes from the materialized workspace at
        // startup, but their bindings and persisted CRDT bytes are still on
        // disk. Keying restore off `workspace.schemes` would drop those
        // documents on the next full CRDT save while their pull cursors stay
        // current -- an empty pull then claims success forever.
        let mut ordered: Vec<SchemeId> = workspace
            .scheme_sync
            .iter()
            .filter_map(|(id, meta)| (meta.kind == SyncDocumentKind::Scheme).then_some(*id))
            .collect();
        ordered.sort();
        for id in &ordered {
            let meta = scheme_meta(&workspace, *id)?;
            let state = states
                .get(&meta.id)
                .map(AsRef::as_ref)
                // Yrs encodes an empty document as the canonical two-byte
                // update `[0, 0]`. It is not a usable persisted snapshot: for
                // a deferred scheme it would later be re-emitted as a
                // schema-less full snapshot, and the backend correctly rejects
                // that with `crdt_schema_invalid`. Treat both that form and a
                // zero-length blob as missing. Materialized schemes still get
                // an empty live document below so normal reconciliation can
                // rebuild their schema from the plain workspace.
                .filter(|state: &&[u8]| !update_v1_is_empty(state));
            if workspace.schemes.contains_key(id) && !defer_materialized_schemes {
                // The loader materialized this scheme (every scheme on desktop;
                // the visible date window on mobile), so decoding it is already
                // required. Restore its bytes, or start an empty base for
                // first-sight/heal — exactly as before.
                let doc = YrsSchemeDocument::for_replica(meta.id, None);
                if let Some(state) = state {
                    doc.apply_update_v1(state)
                        .with_context(|| format!("restore scheme CRDT state {id}"))?;
                }
                schemes.insert(*id, doc);
            } else if let Some(state) = state {
                // Keep persisted bytes verbatim and decode on demand
                // (`hydrate_deferred`). On mobile this includes materialized
                // ordinary schemes: the plain workspace is already loaded for
                // the UI, and decoding its unchanged Yjs history at cold open
                // buys nothing until the scheme is touched. On desktop this
                // branch remains the historical off-window-daily path.
                deferred.insert(
                    *id,
                    DeferredSchemeDocument {
                        document: meta.id,
                        state_v1: Arc::from(state),
                    },
                );
            } else if workspace.schemes.contains_key(id) {
                // A materialized scheme with no persisted bytes is a genuine
                // first-sight/local document. Keep the historical empty live
                // base so the normal reconcile path can author its content;
                // only an existing state is eligible for lazy deferral.
                schemes.insert(*id, YrsSchemeDocument::for_replica(meta.id, None));
            }
            // A binding with neither a materialized scheme nor persisted bytes
            // gets NO local document — same as the historical behaviour, which
            // iterated `workspace.schemes` only. An empty live document here
            // would re-queue a schema-less delta the server rejects forever.
        }
        Ok(Self {
            workspace: workspace_doc,
            schemes,
            deferred,
        })
    }

    /// The set of document IDs this instance owns a persisted CRDT state for —
    /// live *and* deferred. Used by the engine to detect scheme documents that
    /// are in the workspace index but have no local CRDT representation (so their
    /// cursor can be reset). A deferred daily is owned, just not decoded, so it
    /// must count here or every caught-up pull would reset its cursor and
    /// re-download it.
    pub fn known_document_ids(&self) -> std::collections::HashSet<DocumentId> {
        let mut ids = std::collections::HashSet::new();
        ids.insert(self.workspace.id);
        for doc in self.schemes.values() {
            ids.insert(doc.id);
        }
        for deferred in self.deferred.values() {
            ids.insert(deferred.document);
        }
        ids
    }

    /// Whether the durable workspace-index document contains a real persisted
    /// state. A newly signed-in device has an intentionally empty local CRDT;
    /// that empty base must pull the server before any plain-workspace repair
    /// is considered authoritative.
    pub fn workspace_is_seeded(&self) -> bool {
        self.workspace.is_seeded()
    }

    /// Compare the persisted folder records without treating their derived
    /// child lists as local authority. Child lists can legitimately differ
    /// while a concurrent scheme/index update is still being merged; the
    /// folder records themselves are the persistence-boundary signal for a
    /// locally-created or locally-renamed/moved folder.
    pub fn workspace_folder_records_match(&self, workspace: &Workspace) -> anyhow::Result<bool> {
        if !self.workspace.is_seeded() {
            return Ok(false);
        }
        let mut normalized = workspace.clone();
        normalized.ensure_sync_metadata();
        let actual = self.workspace.snapshot()?;
        let mut expected = workspace_document_snapshot(&normalized);
        for folder in &mut expected.folders {
            folder.children.clear();
        }
        let mut actual_folders = actual.folders;
        actual_folders
            .iter_mut()
            .for_each(|folder| folder.children.clear());
        Ok(actual_folders == expected.folders
            && actual.recently_deleted_folders == expected.recently_deleted_folders
            && actual.deleted_folder_origins == expected.deleted_folder_origins
            && actual.folder_sync == expected.folder_sync)
    }

    /// Promote a deferred scheme document into a live Yjs document, so it can be
    /// edited, receive a remote merge, or be materialized. A no-op when the
    /// scheme is already live or not owned at all. A decode failure (bytes also
    /// damaged on disk) is reported but not fatal — the scheme is left absent
    /// from `schemes` so the caller's first-sight path seeds it from an empty
    /// base and the engine's re-convergence resets its cursor.
    pub(crate) fn hydrate_deferred(&mut self, scheme_id: SchemeId) {
        let Some(deferred) = self.deferred.remove(&scheme_id) else {
            return;
        };
        match deferred_live_document(&deferred) {
            Ok(doc) => {
                self.schemes.insert(scheme_id, doc);
            }
            Err(err) => {
                eprintln!(
                    "knotq: deferred scheme document {scheme_id} could not be hydrated \
                     ({err:#}); it will be rebuilt from the server"
                );
            }
        }
    }

    /// Decode every deferred scheme. This is reserved for a full integrity
    /// proof/recovery pass; ordinary startup and websocket wakes intentionally
    /// leave untouched documents as raw persisted bytes.
    pub(crate) fn hydrate_all_deferred(&mut self) {
        let mut scheme_ids: Vec<SchemeId> = self.deferred.keys().copied().collect();
        scheme_ids.sort();
        for scheme_id in scheme_ids {
            self.hydrate_deferred(scheme_id);
        }
    }

    /// Recover one scheme's content from its durable CRDT bytes. The mobile UI
    /// calls this when a lazy Daily Queue file fails to parse: the persisted
    /// CRDT state is intact, so decoding that one document restores it into
    /// `schemes`, from where the next materialization puts it back in the UI
    /// workspace and the following save rewrites a good file. Unrelated deferred
    /// documents are untouched. Returns whether the scheme was deferred (so a
    /// recovery was attempted).
    pub fn request_deferred_recovery(&mut self, scheme_id: SchemeId) -> bool {
        if !self.deferred.contains_key(&scheme_id) {
            return false;
        }
        self.hydrate_deferred(scheme_id);
        true
    }

    /// Hydrate a deferred scheme when the server's startup integrity proof says
    /// its local history differs. Integrity reports document ids, while the
    /// lazy store is keyed by scheme ids, so resolve that mapping here.
    pub fn hydrate_deferred_document(&mut self, document: DocumentId) -> bool {
        let Some(scheme_id) = self.deferred.iter().find_map(|(scheme_id, deferred)| {
            (deferred.document == document).then_some(*scheme_id)
        }) else {
            return false;
        };
        self.hydrate_deferred(scheme_id);
        true
    }

    /// Whether a scheme is currently held only as undecoded deferred bytes.
    /// Exposed so a caller can assert the lazy path is actually being exercised.
    pub fn is_deferred(&self, scheme_id: SchemeId) -> bool {
        self.deferred.contains_key(&scheme_id)
    }

    /// Retain a complete server snapshot for a deferred scheme without
    /// hydrating it into a live Yjs document. This is the fast path for an
    /// off-window Daily Queue page on mobile: the batched server response is
    /// already a complete merged state, so decoding a cold historical page just
    /// to merge it into the same bytes would do work that cannot affect the
    /// current UI. Deltas and visible documents must still use the normal merge
    /// path because they need the existing Yjs history or immediate
    /// materialization.
    ///
    /// Returns `true` only when the retained bytes changed. A document that is
    /// not deferred is deliberately left alone so callers cannot accidentally
    /// replace a live document (which may contain local edits).
    pub fn replace_deferred_full_state(&mut self, document: DocumentId, state_v1: &[u8]) -> bool {
        let Some((_, deferred)) = self
            .deferred
            .iter_mut()
            .find(|(_, deferred)| deferred.document == document)
        else {
            return false;
        };
        if deferred.state_v1.as_ref() == state_v1 {
            return false;
        }
        deferred.state_v1 = Arc::from(state_v1);
        true
    }

    /// Whether `document` is owned only as lazy, undecoded bytes. Deferred
    /// documents intentionally omit a state vector from integrity probes; a
    /// mismatch for one is expected and must not trigger a pull loop. A live
    /// document, including an empty shell, is different: it participates in
    /// integrity checks and can be repaired by re-pulling.
    pub fn owns_deferred_document(&self, document: DocumentId) -> bool {
        self.deferred
            .values()
            .any(|deferred| deferred.document == document)
    }

    /// Counts of decoded vs deferred scheme documents, for structural tests and
    /// load-cost assertions: cold restore and a caught-up pull must keep the
    /// deferred count proportional to the user's history rather than decoding it.
    pub fn document_population(&self) -> CrdtDocumentPopulation {
        CrdtDocumentPopulation {
            live_schemes: self.schemes.len(),
            deferred_schemes: self.deferred.len(),
        }
    }

    /// Compact state-vector proofs for the *decoded* documents this replica
    /// owns. Deferred (lazy) documents are intentionally omitted: computing a
    /// state vector means decoding the document, and this probe runs on the
    /// first pull of every session — decoding the user's whole daily history
    /// there is exactly the cost lazy-loading exists to avoid. A deferred
    /// document damaged on disk is instead caught when it is hydrated (a view,
    /// a remote update, or `request_deferred_recovery`), where the failure
    /// routes into a server-backed rebuild.
    pub fn state_vectors_v1(&self) -> HashMap<DocumentId, Vec<u8>> {
        let mut out = HashMap::with_capacity(self.schemes.len() + 1);
        out.insert(self.workspace.id, self.workspace.state_vector_v1());
        for doc in self.schemes.values() {
            out.insert(doc.id, doc.state_vector_v1());
        }
        out
    }

    /// Derive state vectors from every persisted document without materializing
    /// deferred scheme trees. Yrs only decodes the update's block metadata for
    /// this operation, so mobile can seed its durable startup-proof cache at
    /// cold open without paying the cost of constructing 100+ historical daily
    /// documents. Invalid deferred bytes are omitted and will be handled by the
    /// normal cursor/materialization recovery path.
    pub fn persisted_state_vectors_v1(&self) -> HashMap<DocumentId, Vec<u8>> {
        let mut out = self.state_vectors_v1();
        for deferred in self.deferred.values() {
            if let Ok(state_vector_v1) = yrs::encode_state_vector_from_update_v1(&deferred.state_v1)
            {
                out.insert(deferred.document, state_vector_v1);
            }
        }
        out
    }

    /// State-vector proof for a selected set of documents. Hydrates only the
    /// selected deferred schemes, which lets the post-push proof verify the
    /// documents that were just changed without decoding the whole workspace.
    pub fn state_vectors_v1_for_documents(
        &mut self,
        documents: &HashSet<DocumentId>,
    ) -> HashMap<DocumentId, Vec<u8>> {
        let deferred_ids: Vec<SchemeId> = self
            .deferred
            .iter()
            .filter_map(|(scheme_id, deferred)| {
                documents.contains(&deferred.document).then_some(*scheme_id)
            })
            .collect();
        for scheme_id in deferred_ids {
            self.hydrate_deferred(scheme_id);
        }

        let mut out = HashMap::with_capacity(documents.len());
        if documents.contains(&self.workspace.id) {
            out.insert(self.workspace.id, self.workspace.state_vector_v1());
        }
        for doc in self.schemes.values() {
            if documents.contains(&doc.id) {
                out.insert(doc.id, doc.state_vector_v1());
            }
        }
        out
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
        // Deferred documents are re-emitted from the bytes they were loaded
        // with, unchanged. This is what makes a normal save safe while old
        // dailies are lazy: their persisted state passes straight through
        // rather than being swept because no live document produced it.
        for deferred in self.deferred.values() {
            out.insert(deferred.document, Arc::clone(&deferred.state_v1));
        }
        out
    }

    /// Persisted states for the given schemes' documents only, as bytes.
    ///
    /// For a caller whose durable save is synchronous on its own thread and so
    /// has nowhere to defer the encoding to — the mobile core, which writes the
    /// changed scheme documents inline rather than handing them to a background
    /// task. The desktop wants [`Self::document_state_handles_for`] instead: its
    /// save encodes in the background, and the changed document is precisely the
    /// expensive one, so returning its bytes here would put that encode back on
    /// the UI thread.
    ///
    /// Schemes without a document are skipped rather than reported: a caller
    /// naming one that has since gone away wants the rest saved, not an error.
    pub fn scheme_document_states(
        &self,
        scheme_ids: &HashSet<SchemeId>,
    ) -> HashMap<DocumentId, Arc<[u8]>> {
        let mut out = HashMap::with_capacity(scheme_ids.len());
        for scheme_id in scheme_ids {
            if let Some(doc) = self.schemes.get(scheme_id) {
                out.insert(doc.id, doc.encode_state_shared_v1());
            } else if let Some(deferred) = self.deferred.get(scheme_id) {
                // Defensive: a deferred document is never in a dirty set (it is
                // not editable while deferred), but if one is named, pass its
                // bytes through rather than dropping it.
                out.insert(deferred.document, Arc::clone(&deferred.state_v1));
            }
        }
        out
    }

    /// Handles for just these documents, for a caller that knows which ones it
    /// changed and does not need to rewrite the rest.
    ///
    /// Handles rather than bytes on purpose: the changed document is precisely
    /// the expensive one to encode, so returning its state here would put the
    /// encode back on whichever thread owns the documents — the UI thread —
    /// which is the cost [`Self::document_state_handles`] exists to avoid.
    ///
    /// Ids this workspace does not hold are skipped rather than reported: a
    /// caller naming a document that has since gone away wants the rest saved,
    /// not an error.
    pub fn document_state_handles_for(
        &self,
        documents: &HashSet<DocumentId>,
    ) -> HashMap<DocumentId, DocumentStateHandle> {
        let mut out = HashMap::with_capacity(documents.len());
        if documents.contains(&self.workspace.id) {
            out.insert(self.workspace.id, self.workspace.state_handle());
        }
        for doc in self.schemes.values() {
            if documents.contains(&doc.id) {
                out.insert(doc.id, doc.state_handle());
            }
        }
        // Desktop (the only caller) loads every scheme, so `deferred` is empty
        // there; this pass exists so no document is silently dropped if that
        // ever changes. Decoding is confined to the requested deferred ids.
        for deferred in self.deferred.values() {
            if documents.contains(&deferred.document) {
                if let Ok(doc) = deferred_live_document(deferred) {
                    out.insert(deferred.document, doc.state_handle());
                }
            }
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
        // Empty on desktop (its loader materializes every scheme). Handled for
        // correctness so a full-scope save can never drop a deferred document.
        for deferred in self.deferred.values() {
            if let Ok(doc) = deferred_live_document(deferred) {
                out.insert(deferred.document, doc.state_handle());
            }
        }
        out
    }

    /// A full-state update for every owned document, taken from the live documents
    /// (so it carries their real clientID and clocks). Used by account-switch
    /// recovery and explicit full-snapshot callers. The ordinary bootstrap path
    /// should use [`Self::full_snapshot_updates_for_documents`] so a one-document
    /// edit does not walk every CRDT in the workspace.
    pub fn full_snapshot_updates(&self) -> WorkspaceCrdtSyncOutcome {
        self.full_snapshot_updates_for_documents(&self.known_document_ids())
    }

    /// Produce full snapshots only for the requested document ids.
    ///
    /// Bootstrap normally needs snapshots for documents whose server sequence is
    /// zero (or for every document during an account reseed), not for documents
    /// that already have a server base. Keeping this filter here makes the routine
    /// sync cost proportional to missing bases instead of total workspace size.
    pub fn full_snapshot_updates_for_documents(
        &self,
        documents: &HashSet<DocumentId>,
    ) -> WorkspaceCrdtSyncOutcome {
        let mut outcome = WorkspaceCrdtSyncOutcome::default();
        if documents.contains(&self.workspace.id) {
            outcome.updates.push(CrdtDocumentUpdate {
                document: self.workspace.id,
                kind: self.workspace.kind,
                update_v1: self.workspace.encode_state_v1(),
                touched_items: Vec::new(),
            });
        }
        // Emit schemes in a stable order. `self.schemes` is a HashMap, whose
        // iteration order is randomized per process; the caller queues these as
        // pending edits, so an unstable order gives edits nondeterministic
        // local-sequence numbers. Content still converges either way (CRDT merges
        // commute), but a deterministic order keeps a fuzz seed — and any
        // real-world "reproduce the exact push sequence" investigation —
        // reproducible run to run.
        let mut docs: Vec<&YrsSchemeDocument> = self
            .schemes
            .values()
            .filter(|doc| documents.contains(&doc.id))
            .collect();
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
        // Deferred documents carry a full `state_v1` snapshot already, so they
        // re-seed a base-less server without being decoded. `touched_items` is
        // empty: a deferred document has no local pending edits (any edit would
        // have hydrated it), so the epoch-adoption rescue has nothing to keep.
        let mut deferred: Vec<&DeferredSchemeDocument> = self
            .deferred
            .values()
            .filter(|entry| documents.contains(&entry.document))
            .collect();
        deferred.sort_by_key(|entry| entry.document);
        for entry in deferred {
            outcome.updates.push(CrdtDocumentUpdate {
                document: entry.document,
                kind: SyncDocumentKind::Scheme,
                update_v1: entry.state_v1.to_vec(),
                touched_items: Vec::new(),
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

    /// Like [`Self::reidentify_workspace_document`], but for a document that
    /// was populated deterministically (see [`Self::populate_workspace_if_unpopulated`])
    /// from content that still carried this replica's own pre-canonicalization
    /// identity rather than the account's. A plain re-key only rebinds the
    /// document; the population inside it is still hashed under the wrong
    /// identity, so it can never deduplicate with another replica's population
    /// of the same logical content. This rebuilds the population under
    /// `canonical_base` (the same content, canonicalized) and re-applies
    /// `edited_canonical` (this replica's current content, canonicalized) as an
    /// ordinary edit on top — see [`YrsJsonDocument::repopulate_canonically`].
    pub fn repopulate_workspace_canonically(
        &mut self,
        canonical_base: &Workspace,
        edited_canonical: &Workspace,
        new_id: DocumentId,
    ) -> anyhow::Result<()> {
        let canonical_snapshot = workspace_document_snapshot(canonical_base);
        let edited_snapshot = workspace_document_snapshot(edited_canonical);
        let fresh =
            self.workspace
                .repopulate_canonically(&canonical_snapshot, &edited_snapshot, new_id)?;
        self.workspace = fresh;
        Ok(())
    }

    /// Whether the workspace-index payload changes between two materialized
    /// workspaces. Scheme item edits intentionally do not count: those belong
    /// to scheme content documents and must not cause a redundant workspace
    /// snapshot during first-sync identity adoption.
    pub fn workspace_document_differs(&self, left: &Workspace, right: &Workspace) -> bool {
        workspace_document_snapshot(left) != workspace_document_snapshot(right)
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
        let candidates: HashSet<DocumentId> = self
            .known_document_ids()
            .into_iter()
            .filter(|document| should_heal(*document))
            .collect();
        self.heal_schema_invalid_documents_for_documents(workspace, &candidates)
    }

    /// Heal only the selected documents whose state is schema-invalid.
    ///
    /// The server-head map already tells the caller which documents can accept a
    /// first snapshot. Restricting validation and encoding to that set keeps
    /// routine sync proportional to missing bases instead of total workspace size.
    pub fn heal_schema_invalid_documents_for_documents(
        &mut self,
        workspace: &Workspace,
        candidates: &HashSet<DocumentId>,
    ) -> Vec<DocumentId> {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        let mut healed = Vec::new();
        let state_is_invalid = |kind: SyncDocumentKind, state: Vec<u8>| {
            validate_crdt_update_sequence(kind, [state.as_slice()]).is_err()
        };
        if candidates.contains(&self.workspace.id)
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
            if !candidates.contains(&doc.id)
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

        // An adopted document replaces whatever we held — including a deferred
        // entry for the same scheme (a squash of an off-window daily). Drop it
        // so `document_states` does not later re-emit the pre-squash bytes.
        self.deferred.remove(&scheme_id);
        self.schemes.insert(scheme_id, adopted);
        // Only the adopted document carries fresh authoritative state; an empty
        // CRDT document for any other scheme still means "not flushed here yet".
        let trust_empty = |id: &SchemeId| *id == scheme_id;
        let workspace = self
            .materialized_workspace_repair(current, &trust_empty)
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
        candidates.sort_by_key(|left| std::cmp::Reverse(left.1));
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
        // A deferred document survives as long as its scheme still has a Scheme
        // binding — the same ownership rule as `apply_remote_updates`. A
        // `replace_all` caller passes a fully materialized workspace (desktop),
        // so in practice nothing is deferred here; keep the rule anyway.
        self.deferred.retain(|id, _| {
            workspace
                .scheme_sync
                .get(id)
                .is_some_and(|meta| meta.kind == SyncDocumentKind::Scheme)
        });
        // Sorted: a document created here takes a fresh random clientID, so
        // HashMap iteration order would decide which scheme receives which id.
        // Content converges either way, but a seeded run must replay exactly.
        let mut ordered: Vec<SchemeId> = workspace.schemes.keys().copied().collect();
        ordered.sort();
        for id in &ordered {
            let scheme = &workspace.schemes[id];
            let meta = scheme_meta(&workspace, *id)?;
            // If this scheme was deferred, decode its real bytes before writing
            // to it, so its CRDT history is kept rather than replaced by a diff
            // against an empty base.
            self.hydrate_deferred(*id);
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
        self.sync_changes_with_bases(workspace, changeset, &HashMap::new())
    }

    /// [`Self::sync_changes`], given what each scheme held before the edits being
    /// written, for schemes whose documents had never been populated when those
    /// edits were made (see [`Self::scheme_document_is_unpopulated`]). Such a
    /// document is populated from its base first, so the edits land as edits.
    pub fn sync_changes_with_bases(
        &mut self,
        workspace: &Workspace,
        changeset: &WorkspaceCrdtChangeSet,
        bases: &HashMap<SchemeId, Scheme>,
    ) -> WorkspaceCrdtSyncOutcome {
        self.sync_changes_with_bases_and_workspace_base(workspace, changeset, bases, None)
    }

    /// [`Self::sync_changes_with_bases`], additionally given what the WHOLE
    /// workspace held before the edits being written, for when the
    /// workspace-index document itself had never been populated when those
    /// edits were made (see [`Self::workspace_document_is_unpopulated`]). The
    /// index is then populated from `workspace_base` first (deterministic
    /// clientID, see [`Self::populate_workspace_if_unpopulated`]), so the
    /// edits land as an ordinary delta on top instead of racing another
    /// device's independent population of the same base content.
    pub fn sync_changes_with_bases_and_workspace_base(
        &mut self,
        workspace: &Workspace,
        changeset: &WorkspaceCrdtChangeSet,
        bases: &HashMap<SchemeId, Scheme>,
        workspace_base: Option<&Workspace>,
    ) -> WorkspaceCrdtSyncOutcome {
        let workspace_was_unpopulated = if let Some(base) = workspace_base {
            match self.populate_workspace_if_unpopulated(base) {
                Ok(populated) => populated,
                Err(err) => {
                    let mut outcome = WorkspaceCrdtSyncOutcome::default();
                    outcome.push_error("workspace CRDT population", err);
                    return outcome;
                }
            }
        } else {
            false
        };
        let mut outcome = self.sync_changes_with_scheme_factory(
            workspace,
            changeset,
            bases,
            move |_, document_id| YrsSchemeDocument::for_replica(document_id, None),
        );
        // Population happens before the incremental write, but the captured
        // delta only contains the latter. A pending update must be usable by a
        // fresh peer that has never seen this workspace document, so promote
        // the workspace update to the complete post-edit state when this was
        // the first population. This is still idempotent on an existing peer:
        // the update contains the same Yrs structs, not a second population.
        if workspace_was_unpopulated {
            if let Some(update) = outcome
                .updates
                .iter_mut()
                .find(|update| update.kind == SyncDocumentKind::PersonalWorkspace)
            {
                update.update_v1 = self.workspace.encode_state_v1();
                update.touched_items.clear();
            }
        }
        outcome
    }

    /// Whether `scheme`'s document has never been populated on this replica —
    /// absent, or present but empty. An edit to such a scheme has to record the
    /// scheme's content from before the edit and hand it to
    /// [`Self::sync_changes_with_bases`]. A deferred document holds real bytes.
    pub fn scheme_document_is_unpopulated(&self, scheme: SchemeId) -> bool {
        if self.deferred.contains_key(&scheme) {
            return false;
        }
        self.schemes
            .get(&scheme)
            .is_none_or(|document| document.is_unpopulated())
    }

    /// Whether the workspace-index document has never been populated on this
    /// replica — see [`Self::scheme_document_is_unpopulated`], same idea one
    /// level up.
    pub fn workspace_document_is_unpopulated(&self) -> bool {
        !self.workspace.is_seeded()
    }

    /// Populate the workspace-index document from `workspace`'s current
    /// content, if it has never been populated — a no-op otherwise. Uses a
    /// deterministic, content-derived clientID (see
    /// `YrsJsonDocument::populate`) rather than leaving the document for
    /// whatever writes to it first to pick a random one.
    ///
    /// Returns whether it actually populated the document (`false` when it
    /// was already seeded) — the caller needs this to treat the document like
    /// a heal: any pending delta queued against the pre-population (empty)
    /// state vector is stale once this rewrites it from scratch.
    pub fn populate_workspace_if_unpopulated(&self, workspace: &Workspace) -> anyhow::Result<bool> {
        if self.workspace_document_is_unpopulated() {
            self.workspace
                .populate(&workspace_document_snapshot(workspace))?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Write the given schemes' content documents from `workspace`, and nothing
    /// else: the workspace index is never re-emitted and no other document is
    /// dropped. For repairs that re-express local scheme content against
    /// documents that may hold newer remote state. `sync_changes` treats every
    /// scheme the passed workspace does not list as removed — so given a
    /// workspace from before a pull it re-wrote the whole index from that stale
    /// copy, deleting the folders, schemes and days the pull had just brought
    /// in, and pruned their documents.
    pub fn sync_scheme_documents(
        &mut self,
        workspace: &Workspace,
        schemes: &[SchemeId],
    ) -> WorkspaceCrdtSyncOutcome {
        let workspace = if workspace.sync_metadata_is_current() {
            Cow::Borrowed(workspace)
        } else {
            let mut repaired = workspace.clone();
            repaired.ensure_sync_metadata();
            Cow::Owned(repaired)
        };
        let workspace = workspace.as_ref();
        let mut outcome = WorkspaceCrdtSyncOutcome::default();
        let mut ids: Vec<SchemeId> = schemes.to_vec();
        ids.sort();
        ids.dedup();
        for id in ids {
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
            self.hydrate_deferred(id);
            match self
                .schemes
                .entry(id)
                .or_insert_with(|| YrsSchemeDocument::for_replica(meta.id, None))
                .sync_scheme(scheme)
            {
                Ok(Some(update)) => outcome.updates.push(update),
                Ok(None) => {}
                Err(err) => outcome.push_error(format!("scheme CRDT update {id}"), err),
            }
        }
        outcome
    }

    fn sync_changes_with_scheme_factory(
        &mut self,
        workspace: &Workspace,
        changeset: &WorkspaceCrdtChangeSet,
        bases: &HashMap<SchemeId, Scheme>,
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
            let desired = workspace_document_snapshot(workspace);
            let archive_changed = self
                .workspace
                .snapshot()
                .map(|current| {
                    current.recently_deleted != desired.recently_deleted
                        || current.recently_deleted_folders != desired.recently_deleted_folders
                })
                .unwrap_or(true);
            let force =
                workspace_documents_missing || workspace_documents_removed || archive_changed;
            match self.workspace.sync_snapshot(&desired, force) {
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
                .filter(|id| !self.schemes.contains_key(id) && !self.deferred.contains_key(id)),
        );
        self.schemes
            .retain(|id, _| workspace.schemes.contains_key(id));
        // Deferred documents are pruned by their *sync binding*, never by the
        // (lazy, UI-facing) `workspace.schemes`: an edit's workspace is the
        // mobile view, which omits every off-window daily. `scheme_sync` is the
        // complete index, so this drops only a scheme that was actually removed.
        self.deferred.retain(|id, _| {
            workspace
                .scheme_sync
                .get(id)
                .is_some_and(|meta| meta.kind == SyncDocumentKind::Scheme)
        });
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
            // Editing a daily that was still deferred (it just entered the view
            // window): decode its real bytes so the edit diffs against real
            // history instead of an empty base.
            self.hydrate_deferred(id);
            // Materialization intentionally hides duplicate item ids in losing
            // schemes, but the raw document still owns those copies. A normal
            // local edit to this scheme must not interpret that visibility choice
            // as a deletion. Preserve raw-only items unless this command batch
            // explicitly deleted that id in this scheme; the latter is the only
            // causal evidence needed to emit a tombstone.
            let sync_scheme = self
                .schemes
                .get(&id)
                .and_then(|document| document.scheme_items().ok())
                .map(|raw_items| {
                    merge_raw_only_items(scheme, raw_items, changeset.deleted_items.get(&id))
                })
                .unwrap_or_else(|| scheme.clone());
            match self
                .schemes
                .entry(id)
                .or_insert_with(|| new_scheme_document(id, meta.id))
                .sync_scheme_from_base(bases.get(&id), &sync_scheme)
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
            changed_documents: HashSet::new(),
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
            let apply_result = self.workspace.apply_update_v1(&update.update_v1);
            match apply_result {
                // Only a merge that actually changed the document counts as
                // applied. An echo of this replica's own push (the server
                // broadcasts `changed` to every device, including the origin)
                // merges as a no-op and must not trigger re-materialization —
                // otherwise every local edit bounces back as a phantom "remote
                // change" that rebuilds views and clobbers in-progress editing.
                Ok(true) => {
                    outcome.applied += 1;
                    workspace_applied = true;
                    outcome.changed_documents.insert(update.document);
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

        // No scheme content has been applied yet, so this first pass only
        // reflects the workspace-structure document. A scheme whose live CRDT
        // document is empty here has NOT had a remote update applied, so an
        // empty document cannot be an authoritative deletion — keep `current`'s
        // items (this protects a scheme created locally but not yet flushed to
        // its CRDT document, e.g. the desktop's direct Daily Queue creation).
        // When no workspace update changed the doc (empty batch or pure echo),
        // `outcome.workspace` stays the `current` clone.
        if workspace_update_eligible_for_materialization {
            match self.materialized_workspace_repair(current, &|_| false) {
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

        // A *live* CRDT document is kept only when the materialized workspace
        // actually holds that scheme. A daily-queue entry with no scheme node
        // yet, or a scheme mid-undo, is not materializable: keeping an empty
        // live document for it re-queues a schema-less delta the server rejects
        // forever (`crdt_schema_invalid`), and `heal_schema_invalid_documents`
        // cannot fix it because the scheme is absent from `workspace.schemes`.
        //
        // A live document for a scheme that IS still bound in the index but is
        // not currently materialized (an off-window Daily Queue day: the mobile
        // loader keeps only the visible window live) must NOT just be dropped —
        // it just came down from the server with a full, valid snapshot, and if
        // it is discarded here it is absent from `known_document_ids`, so every
        // subsequent caught-up pull re-fetches and re-applies it forever (the
        // "materialization gap" re-pull loop). Demote it to `deferred` instead —
        // undecoded bytes, exactly as the load path does — so its state
        // survives without a live Yjs document and without being pushed.
        let mut demote_to_deferred: Vec<(SchemeId, DeferredSchemeDocument)> = Vec::new();
        self.schemes.retain(|id, doc| {
            if outcome.workspace.schemes.contains_key(id) {
                return true;
            }
            let bound = outcome
                .workspace
                .scheme_sync
                .get(id)
                .is_some_and(|meta| meta.kind == SyncDocumentKind::Scheme);
            if bound {
                let state_v1 = doc.encode_state_shared_v1();
                if !update_v1_is_empty(state_v1.as_ref()) {
                    demote_to_deferred.push((
                        *id,
                        DeferredSchemeDocument {
                            document: doc.id,
                            state_v1,
                        },
                    ));
                }
            }
            false
        });
        for (scheme_id, deferred) in demote_to_deferred {
            self.deferred.insert(scheme_id, deferred);
        }
        // A *deferred* document is different: it is never materialized into the
        // UI workspace by design, is never pushed as a delta (an edit hydrates
        // it first), and its persisted bytes are already a valid full snapshot.
        // So it is kept as long as its durable sync binding survives — a remote
        // update to one scheme must never sweep the untouched bytes of an
        // unrelated off-window daily.
        self.deferred.retain(|id, _| {
            outcome
                .workspace
                .scheme_sync
                .get(id)
                .is_some_and(|meta| meta.kind == SyncDocumentKind::Scheme)
        });
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
            // A remote update targeting an off-window/visible deferred scheme:
            // decode its durable bytes into a live document first, so the merge
            // lands on real history and the normal materialization path can
            // repair any stale scheme file before a later navigation.
            self.hydrate_deferred(scheme_id);
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
                    outcome.changed_documents.insert(update.document);
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
                        outcome.changed_documents.insert(update.document);
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
            // A scheme this batch touched has just had authoritative remote
            // state merged into its document — trust it even if it is now
            // empty (a remote "delete every item"). For every *other* scheme an
            // empty document still means "content not in the CRDT here yet" and
            // `current`'s items are kept.
            let trust_empty = |scheme_id: &SchemeId| touched_schemes.contains(scheme_id);
            match self.materialized_workspace_repair(current, &trust_empty) {
                Ok(workspace) => outcome.workspace = workspace,
                Err(err) => outcome.push_workspace_error("scheme materialization", err),
            }
        }

        outcome
    }

    /// Rebuild the workspace from the CRDT documents for the ordinary sync path:
    /// scales with the schemes this replica has decoded (ordinary schemes plus
    /// the visible daily window), NOT with the user's whole daily history. An
    /// off-window daily that is still deferred stays out of the result — it is
    /// deliberately not in the UI workspace and its bytes are untouched.
    /// `current` supplies only state the documents do not carry.
    ///
    /// `trust_empty_crdt` decides, per scheme, whether an *empty* live CRDT
    /// document is authoritative (`true` — a real merge result: a delete of
    /// every item, or a repair of a bad local save) or means "content not in
    /// the CRDT here yet" (`false` — a daily just created locally, a scheme
    /// mid-bootstrap), in which case `current`'s items are kept so locally
    /// authored content that has not synced is never wiped by a repair pass.
    pub fn materialized_workspace_repair(
        &self,
        current: &Workspace,
        trust_empty_crdt: &dyn Fn(&SchemeId) -> bool,
    ) -> anyhow::Result<Workspace> {
        Ok(self
            .materialize_workspace_inner(current, false, trust_empty_crdt)?
            .0)
    }

    /// [`materialized_workspace_repair`](Self::materialized_workspace_repair),
    /// also naming the schemes whose document still holds a live copy of a line
    /// the result places in another scheme.
    ///
    /// Those copies are invisible, which is the problem: deleting the visible
    /// one later reveals the hidden one, and a line the user deleted comes back
    /// somewhere else. A caller that can write to the documents should delete
    /// them, passing the map straight back as a change set's `deleted_items`
    /// (an ordinary scheme write preserves raw-only copies on purpose; only an
    /// explicit deletion tombstones one). See `dedupe_materialized_items` for
    /// why this cannot lose the line.
    pub fn materialized_workspace_with_hidden_copies(
        &self,
        current: &Workspace,
        trust_empty_crdt: &dyn Fn(&SchemeId) -> bool,
    ) -> anyhow::Result<(Workspace, HashMap<SchemeId, HashSet<String>>)> {
        self.materialize_workspace_inner(current, false, trust_empty_crdt)
    }

    /// Every scheme document that holds a live copy of `item`.
    ///
    /// An item id is globally unique, so more than one entry means two
    /// documents both believe they own the line — the cross-document duplicate
    /// placement that `dedupe_materialized_items` resolves by lowest scheme id.
    /// Diagnostic: when the visible workspace and the documents disagree about
    /// where a line lives, this says whether the cause is a duplicate (two
    /// entries) or a plain mismatch (one entry, in the wrong place).
    pub fn documents_holding_item(&self, item: knotq_model::ItemId) -> Vec<SchemeId> {
        let mut holders: Vec<SchemeId> = self
            .schemes
            .iter()
            .filter(|(_, document)| {
                document
                    .scheme_items()
                    .map(|items| items.iter().any(|candidate| candidate.id == item))
                    .unwrap_or(false)
            })
            .map(|(id, _)| *id)
            .collect();
        holders.sort();
        holders
    }

    /// Read one live scheme document without applying workspace-wide duplicate
    /// placement dedupe. The pre-pull repair path needs the raw per-document
    /// item set: a copy hidden by materialization is still authoritative CRDT
    /// history and must not be rewritten as a deletion merely because the same
    /// id is visible in another scheme.
    pub(crate) fn raw_scheme_items(
        &self,
        scheme_id: SchemeId,
    ) -> anyhow::Result<Option<Vec<Item>>> {
        self.schemes
            .get(&scheme_id)
            .map(YrsSchemeDocument::scheme_items)
            .transpose()
    }

    /// The exhaustive variant: every deferred daily is decoded too, so the
    /// result is the complete picture of what the CRDT holds. Rebuilding this
    /// and comparing it against disk is how a data directory is checked for
    /// integrity, and how a test oracle confirms nothing diverged — both accept
    /// the cost of touching every historical daily. Not for the sync hot path.
    pub fn materialized_workspace_for_diagnostics(
        &self,
        current: &Workspace,
    ) -> anyhow::Result<Workspace> {
        Ok(self
            .materialize_workspace_inner(current, true, &|_| true)?
            .0)
    }

    /// Rebuild the workspace from the CRDT documents, using `current` only for
    /// state the documents do not carry (a scheme with no local document, and
    /// the local-only calendar sync token). `hydrate_all_deferred` decodes every
    /// deferred daily as well (the diagnostic/oracle path). `trust_empty_crdt`
    /// decides, per scheme, whether an empty live CRDT document is authoritative
    /// (true) or means "content not yet in the CRDT, keep `current`" (false).
    fn materialize_workspace_inner(
        &self,
        current: &Workspace,
        hydrate_all_deferred: bool,
        trust_empty_crdt: &dyn Fn(&SchemeId) -> bool,
    ) -> anyhow::Result<(Workspace, HashMap<SchemeId, HashSet<String>>)> {
        // A workspace document that was never seeded (a fresh device before its
        // first pull, or one whose local CRDT state is empty) describes nothing.
        // `snapshot()` would fail with "workspace id missing"; there is simply
        // nothing to materialize, so hand `current` back unchanged. The caller's
        // `materialized == workspace` check then correctly reports no repair.
        if !self.workspace.is_seeded() {
            return Ok((current.clone(), HashMap::new()));
        }
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

        // `snapshot.schemes` carries every scheme entry the workspace index
        // holds — ordinary schemes *and* Daily Queue schemes (the daily-queue
        // filter only keeps dailies out of the folder tree, not out of this
        // list). So name/colour/source always come from the index here; the
        // only question per entry is where its items come from.
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
            let live_items = self
                .schemes
                .get(&entry.id)
                .and_then(|doc| doc.scheme_items().ok());
            let items = if let Some(items) = live_items
                .as_ref()
                .filter(|items| !items.is_empty() || trust_empty_crdt(&entry.id))
                .cloned()
            {
                // A live document with content, or an empty one the caller
                // trusts as authoritative (a real merge result, or a synced
                // scheme). Locally-authored content that has never reached the
                // CRDT falls through instead of being wiped.
                items
            } else if self.deferred.contains_key(&entry.id) {
                // A deferred Daily Queue document is decoded when it is inside
                // the view window (so a stale on-disk copy is repaired from
                // authoritative CRDT state — bounded by the view window, not
                // total history) or on the exhaustive diagnostic pass.
                //
                // Ordinary mobile schemes are also deferred now, but their
                // plain scheme file is already visible and remains the cheap
                // fallback until that scheme is touched or an integrity proof
                // explicitly asks for its CRDT bytes. This keeps an unrelated
                // remote update from decoding every untouched scheme.
                let visible = current.schemes.contains_key(&entry.id);
                if !visible && !hydrate_all_deferred {
                    continue;
                }
                let is_daily = current.daily_queue.values().any(|id| id == &entry.id);
                if visible && !is_daily && !hydrate_all_deferred {
                    if let Some(scheme) = current.schemes.get(&entry.id) {
                        scheme.items.clone()
                    } else {
                        continue;
                    }
                } else {
                    match deferred_live_document(&self.deferred[&entry.id])
                        .and_then(|doc| doc.scheme_items())
                    {
                        Ok(items) => items,
                        Err(err) => {
                            eprintln!(
                                "knotq: deferred scheme {} could not be decoded ({err:#}); \
                                 using the on-disk copy",
                                entry.id
                            );
                            match current.schemes.get(&entry.id) {
                                Some(scheme) => scheme.items.clone(),
                                None => continue,
                            }
                        }
                    }
                }
            } else if let Some(scheme) = current.schemes.get(&entry.id) {
                scheme.items.clone()
            } else {
                Vec::new()
            };
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

        // A loaded scheme can be absent from the merged `nodes` map while its
        // durable `scheme_sync` binding and Daily Queue index entry survive.
        // This happens when a device that has the page loaded adopts an index
        // snapshot produced by a lazy device: the lazy snapshot intentionally
        // omits the page body, but it must not make an already-loaded page
        // disappear from the receiving workspace. Keep the current metadata
        // and prefer the authoritative CRDT body when one is available.
        let retained_loaded_schemes: Vec<SchemeId> = current
            .schemes
            .keys()
            .filter(|id| {
                !workspace.schemes.contains_key(id)
                    && workspace
                        .scheme_sync
                        .get(id)
                        .is_some_and(|sync| sync.kind == SyncDocumentKind::Scheme)
            })
            .copied()
            .collect();
        for scheme_id in retained_loaded_schemes {
            let Some(current_scheme) = current.schemes.get(&scheme_id) else {
                continue;
            };
            let items = if let Some(items) = self
                .schemes
                .get(&scheme_id)
                .and_then(|document| document.scheme_items().ok())
            {
                items
            } else if let Some(deferred) = self.deferred.get(&scheme_id) {
                deferred_live_document(deferred)
                    .and_then(|document| document.scheme_items())
                    .unwrap_or_else(|_| current_scheme.items.clone())
            } else {
                current_scheme.items.clone()
            };
            workspace.schemes.insert(
                scheme_id,
                Scheme {
                    id: scheme_id,
                    name: current_scheme.name.clone(),
                    color_index: current_scheme.color_index,
                    gsync: current_scheme.gsync,
                    source: current_scheme.source.clone(),
                    items,
                },
            );
        }

        // An item id is globally unique. A concurrent move is represented as
        // a tombstone in the source document plus a live insert in the target;
        // when two devices choose different targets, both inserts are otherwise
        // valid and would materialize as two copies. Keep the same deterministic
        // winner on every replica. Only schemes materialized above participate:
        // a lazy/off-window Daily page is intentionally absent and must not be
        // interpreted as a deletion or placement decision.
        let hidden_copies = dedupe_materialized_items(&mut workspace);

        workspace.ensure_sync_metadata();
        Ok((workspace, hidden_copies))
    }
}

/// Add live items present in the raw scheme document but hidden from the plain
/// workspace by cross-document duplicate-id materialization. Keep the local
/// workspace's order for visible items, and project each hidden item's prior raw
/// position into that list so a later source deletion reveals it in a stable spot.
pub(crate) fn merge_raw_only_items(
    scheme: &Scheme,
    raw_items: Vec<Item>,
    deleted_items: Option<&HashSet<String>>,
) -> Scheme {
    let local_ids: HashSet<String> = scheme
        .items
        .iter()
        .map(|item| item.id.to_string())
        .collect();
    let deleted_items = deleted_items.cloned().unwrap_or_default();
    let mut extras: Vec<(usize, Item)> = raw_items
        .into_iter()
        .enumerate()
        .filter(|(_, item)| {
            let id = item.id.to_string();
            !local_ids.contains(&id) && !deleted_items.contains(&id)
        })
        .collect();
    if extras.is_empty() {
        return scheme.clone();
    }

    let mut merged = scheme.clone();
    // Insert from right to left so each raw position is measured against the
    // original visible list rather than being shifted by an earlier insert.
    extras.sort_by_key(|(position, _)| *position);
    for (position, item) in extras.into_iter().rev() {
        merged.items.insert(position.min(merged.items.len()), item);
    }
    merged
}

/// Resolve cross-document duplicate placements, reporting the schemes a copy
/// was hidden in.
///
/// The winner is the lowest scheme id, which is arbitrary but identical on
/// every replica — and, because it is a minimum, the copy in the globally
/// lowest scheme is never a loser anywhere. A caller may therefore delete the
/// losing copies from their documents without any risk of every replica
/// deleting a different one and losing the line altogether.
fn dedupe_materialized_items(workspace: &mut Workspace) -> HashMap<SchemeId, HashSet<String>> {
    let mut scheme_ids: Vec<SchemeId> = workspace.schemes.keys().copied().collect();
    scheme_ids.sort();
    let mut seen = HashSet::new();
    let mut hidden: HashMap<SchemeId, HashSet<String>> = HashMap::new();
    for scheme_id in scheme_ids {
        let Some(scheme) = workspace.schemes.get_mut(&scheme_id) else {
            continue;
        };
        scheme.items.retain(|item| {
            if seen.insert(item.id) {
                return true;
            }
            hidden
                .entry(scheme_id)
                .or_default()
                .insert(item.id.to_string());
            false
        });
    }
    hidden
}

impl WorkspaceCrdtDocuments {}

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
    // A deferred document is owned, just not decoded — it is not "missing", and
    // treating it as such would force a full workspace snapshot on every edit
    // made while an old daily is still lazy.
    workspace
        .schemes
        .keys()
        .any(|id| !docs.schemes.contains_key(id) && !docs.deferred.contains_key(id))
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
    /// Field-schema stamp, present only on entries written by a build that also
    /// maintains the `node_fields` map. Its ABSENCE is the signal that matters:
    /// a build predating `node_fields` regenerates this whole entry from its own
    /// struct on every write, so it can never carry the stamp, and its payload is
    /// therefore the authority for that node. See `NODE_FIELD_SCHEMA`.
    ///
    /// `skip_serializing_if` keeps it off the wire when absent so an entry an old
    /// build wrote and a new build merely re-reads stays byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field_schema: Option<u32>,
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
