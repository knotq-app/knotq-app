//! Shared in-memory, engine-driven multi-device test harness.
//!
//! There is **no network**: [`TestServer`] implements the real [`SyncTransport`]
//! trait against an in-process `HashMap`, mirroring the production worker's
//! merged-state model — one merged Yjs `state_v1` per document, bumped by a `seq`
//! on each push. Devices sync through the *actual* shared engine
//! ([`batch_pull_and_apply`] + [`batch_push_pending`]) and the real CRDT layer, so
//! these tests exercise exactly the code desktop and mobile run, end to end.
//!
//! ## Backend-agnostic harness
//!
//! [`Harness::new`] creates an in-memory harness (no network). [`Harness::new_http`]
//! creates an HTTP harness that runs the SAME scenario code against the real
//! Cloudflare Worker backend. The two constructors share all the operation methods.
//! Server-introspection knobs that only exist in-memory (reject_next_push_with_schema_invalid,
//! server_document_count, etc.) panic when called on an HTTP harness.
//!
//! ## Backend atomicity semantics (from `backend/cloudflare/src/index.ts`)
//!
//! `handleSyncPush` iterates over documents inside a single
//! `this.state.storage.transactionSync(() => { … })` call.  Any throw inside that
//! closure — including an `ApiError(400, "crdt_schema_invalid")` thrown by
//! `validateAndCompactCrdtUpdates` for any document in the batch — aborts the whole
//! transaction.  **No documents from that batch are persisted.**  This is a
//! fully-atomic, all-or-nothing batch rejection.  `TestServer::push` replicates this
//! exactly: it validates every document before writing any, and returns
//! `Err("sync backend rejected request: crdt_schema_invalid")` if any document
//! fails, leaving the server state unchanged.

#![allow(dead_code)]

pub mod http_transport;
pub mod rich_items;
pub mod scenarios;

mod harness;
mod summaries;
mod test_device;
mod test_device_ops;
mod test_device_sync;
mod test_server;
mod util;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use anyhow::anyhow;
use chrono::{Duration, NaiveDate, Utc};
use knotq_model::{
    daily_queue_displaced_item_id, daily_queue_scheme_id, CalendarProvider, DocumentId, Folder,
    FolderId, ImageAssetFormat, ImageInline, ImportedCalendarSource, Item, ItemId, ItemMarker,
    NodeRef, OperationId, ReplicaId, Scheme, SchemeId, SchemeSource, SyncDocumentKind, Workspace,
    WorkspaceId,
};
use knotq_sync::{
    batch_pull_and_apply, batch_push_pending, queue_workspace_bootstrap_updates,
    validate_crdt_update_sequence, BatchPullRequest, BatchPullResponse, BatchPushRequest,
    BatchPushResponse, CrdtDocumentUpdate, LocalSyncState, NotificationScheduleSnapshot,
    PendingCrdtEdit, PulledCrdtDocument, PushDocumentUpdates, PushedCrdtDocument, SyncPushRejected,
    SyncTransport, WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments, MAX_SYNC_MEDIA_BYTES,
};
use uuid::Uuid;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use summaries::{
    item_summary, node_ref_label, scheme_source_label, FolderSummary, SchemeSummary,
    WorkspaceSummary,
};
pub use test_server::TestServer;
pub use util::Rng;
use util::{
    dq_item_is_fully_complete_task, dq_last_nonblank_day, dq_scheme_is_blank, dq_strip_annotations,
    merge_state, test_notification_schedule,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct DeviceKey(pub usize);

// ---------------------------------------------------------------------------
// Backend abstraction
// ---------------------------------------------------------------------------

/// Distinguishes which backend the Harness is running against.  The HTTP variant
/// holds one HttpClient per device (indexed by DeviceKey); they all share the same
/// workspace but have independent bearer tokens.
enum HarnessBackend {
    InMemory(TestServer),
    Http(HashMap<DeviceKey, http_transport::HttpClient>),
}

pub const D0: DeviceKey = DeviceKey(0);
pub const D1: DeviceKey = DeviceKey(1);
pub const D2: DeviceKey = DeviceKey(2);

fn item_image_assets(item: &Item) -> Vec<ImageInline> {
    let mut images = Vec::new();
    collect_item_image_assets(item, &mut images);
    images
}

fn collect_item_image_assets(item: &Item, images: &mut Vec<ImageInline>) {
    if let Some(image) = item.content.image() {
        images.push(*image);
    }
    if let Some(table) = item.table() {
        for cell in table.cells() {
            for item in &cell.items {
                collect_item_image_assets(item, images);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

pub struct Harness {
    account_workspace: WorkspaceId,
    base: Workspace,
    backend: HarnessBackend,
    devices: BTreeMap<DeviceKey, TestDevice>,
    device_count: usize,
}

// `impl Harness` lives in `harness.rs`.

// ---------------------------------------------------------------------------
// Test server — implements the real SyncTransport against the merged-state model
// (definitions + impls in `test_server.rs`).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test device — drives the real engine
// ---------------------------------------------------------------------------

pub struct TestDevice {
    account_workspace: WorkspaceId,
    replica_id: ReplicaId,
    pub workspace: Workspace,
    // The long-lived CRDT documents that local edits diff against — the desktop
    // `WorkspaceStore.crdt` / mobile `self.crdt`. Faithful to the fixed drivers, it
    // is reconstructed from persisted per-document state with a deterministic
    // clientID (`from_states`), never rebuilt-from-plain-data with a throwaway
    // identity. `crdt_states` is the in-memory stand-in for the on-disk CRDT state
    // file; round-tripping through it every sync exercises the persistence path the
    // real drivers use to hand the documents between restarts and threads.
    store_crdt: WorkspaceCrdtDocuments,
    crdt_states: HashMap<DocumentId, Vec<u8>>,
    local_state: LocalSyncState,
    next_sequence: u64,
    /// In-memory stand-in for the desktop's `media/` assets directory.
    /// Maps image_name (e.g. "<uuid>.png") → raw bytes.  Populated by
    /// [`Self::attach_image`] (local write) and [`Self::download_media_from`].
    pub media_assets: HashMap<String, Vec<u8>>,
    /// Documents skipped during the most recent sync (accumulated across all pull
    /// pages).  Reset at the start of each `try_sync` call.
    pub last_skipped: Vec<knotq_sync::SkippedDocument>,
}

// `impl TestDevice` lives in `test_device.rs` and `test_device_ops.rs`.
