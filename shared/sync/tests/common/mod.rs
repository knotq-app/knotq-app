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

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use anyhow::anyhow;
use chrono::{Duration, NaiveDate, Utc};
use knotq_model::{
    daily_queue_scheme_id, CalendarProvider, DocumentId, Folder, FolderId, ImageAssetFormat,
    ImageInline, ImportedCalendarSource, Item, ItemId, ItemMarker, NodeRef, OperationId, ReplicaId,
    Scheme, SchemeId, SchemeSource, SyncDocumentKind, Workspace, WorkspaceId,
};
use knotq_sync::{
    batch_pull_and_apply, batch_push_pending, queue_workspace_bootstrap_updates,
    validate_crdt_update_sequence, BatchPullRequest, BatchPullResponse, BatchPushRequest,
    BatchPushResponse, LocalSyncState, NotificationScheduleSnapshot, PendingCrdtEdit,
    PulledCrdtDocument, PushDocumentUpdates, PushedCrdtDocument, SyncPushRejected, SyncTransport,
    WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments, MAX_SYNC_MEDIA_BYTES,
};
use uuid::Uuid;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

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

impl Harness {
    pub fn new(device_count: usize) -> Self {
        // Every device on an account shares the same workspace skeleton (same root
        // folder id and sync document id) after its first sync. Model that by
        // cloning one canonical base into each device rather than minting an
        // independent root per device.
        let account_workspace = WorkspaceId::new();
        let mut base = Workspace::new();
        base.canonicalize_personal_sync_identity(account_workspace);
        base.ensure_sync_metadata();
        Self {
            account_workspace,
            base,
            backend: HarnessBackend::InMemory(TestServer::default()),
            devices: BTreeMap::new(),
            device_count,
        }
    }

    /// Create an HTTP harness backed by the real Cloudflare Worker.
    /// `bootstrap_responses` must have exactly `device_count` entries from
    /// separate `backend_bootstrap` calls with the SAME email so they share a
    /// workspace_id.  Each device gets its own HttpClient (bearer token).
    pub fn new_http(base_url: &str, workspace_id: WorkspaceId, bearer_tokens: Vec<String>) -> Self {
        assert!(
            !bearer_tokens.is_empty(),
            "new_http requires at least one bearer token"
        );
        let device_count = bearer_tokens.len();
        let mut base = Workspace::new();
        base.canonicalize_personal_sync_identity(workspace_id);
        base.ensure_sync_metadata();
        let mut clients: HashMap<DeviceKey, http_transport::HttpClient> = HashMap::new();
        for (i, token) in bearer_tokens.into_iter().enumerate() {
            clients.insert(
                DeviceKey(i),
                http_transport::HttpClient {
                    api_base: base_url.trim_end_matches('/').to_string(),
                    bearer_token: token,
                },
            );
        }
        Self {
            account_workspace: workspace_id,
            base,
            backend: HarnessBackend::Http(clients),
            devices: BTreeMap::new(),
            device_count,
        }
    }

    /// True when this harness is backed by the real HTTP backend.
    pub fn is_http(&self) -> bool {
        matches!(self.backend, HarnessBackend::Http(_))
    }

    /// Return a reference to the in-memory TestServer.  Panics when called on an
    /// HTTP harness — in-memory introspection knobs are not available over the wire.
    fn require_in_memory_server(&self) -> &TestServer {
        match &self.backend {
            HarnessBackend::InMemory(server) => server,
            HarnessBackend::Http(_) => {
                panic!("server introspection is only available for the in-memory harness")
            }
        }
    }

    pub fn login_all(&mut self) {
        for i in 0..self.device_count {
            self.devices.insert(
                DeviceKey(i),
                TestDevice::from_base(&self.base, self.account_workspace),
            );
        }
    }

    pub fn device_keys(&self) -> Vec<DeviceKey> {
        self.devices.keys().copied().collect()
    }

    pub fn account_workspace(&self) -> WorkspaceId {
        self.account_workspace
    }

    pub fn device(&self, key: DeviceKey) -> &TestDevice {
        self.devices
            .get(&key)
            .unwrap_or_else(|| panic!("missing device {key:?}"))
    }

    fn device_mut(&mut self, key: DeviceKey) -> &mut TestDevice {
        self.devices
            .get_mut(&key)
            .unwrap_or_else(|| panic!("missing device {key:?}"))
    }

    /// Mutable reference to a device for white-box surgery in regression tests
    /// (e.g. dropping pending edits to simulate a partially-acked push).
    pub fn device_mut_for_surgery(&mut self, key: DeviceKey) -> &mut TestDevice {
        self.device_mut(key)
    }

    // --- operations ---

    pub fn add_scheme(&mut self, key: DeviceKey, name: &str, lines: &[&str]) -> SchemeId {
        self.device_mut(key).add_scheme(name, lines)
    }

    pub fn append_line(&mut self, key: DeviceKey, scheme: SchemeId, text: &str) {
        self.device_mut(key).append_line(scheme, text);
    }

    pub fn edit_line(&mut self, key: DeviceKey, scheme: SchemeId, index: usize, text: &str) {
        self.device_mut(key).edit_line(scheme, index, text);
    }

    pub fn insert_line(&mut self, key: DeviceKey, scheme: SchemeId, index: usize, text: &str) {
        self.device_mut(key).insert_line(scheme, index, text);
    }

    pub fn remove_line(&mut self, key: DeviceKey, scheme: SchemeId, index: usize) {
        self.device_mut(key).remove_line(scheme, index);
    }

    pub fn reorder_reverse(&mut self, key: DeviceKey, scheme: SchemeId) {
        self.device_mut(key).reorder_reverse(scheme);
    }

    pub fn rename_scheme(&mut self, key: DeviceKey, scheme: SchemeId, name: &str) {
        self.device_mut(key).rename_scheme(scheme, name);
    }

    pub fn add_folder(&mut self, key: DeviceKey, name: &str) -> FolderId {
        self.device_mut(key).add_folder(name)
    }

    pub fn move_scheme_to_folder(&mut self, key: DeviceKey, scheme: SchemeId, folder: FolderId) {
        self.device_mut(key).move_scheme_to_folder(scheme, folder);
    }

    pub fn archive_scheme(&mut self, key: DeviceKey, scheme: SchemeId) {
        self.device_mut(key).archive_scheme(scheme);
    }

    pub fn restore_scheme(&mut self, key: DeviceKey, scheme: SchemeId) {
        self.device_mut(key).restore_scheme(scheme);
    }

    pub fn import_calendar_scheme(
        &mut self,
        key: DeviceKey,
        name: &str,
        account_id: &str,
        account_email: &str,
        calendar_id: &str,
        events: &[&str],
    ) -> SchemeId {
        self.device_mut(key).import_calendar_scheme(
            name,
            account_id,
            account_email,
            calendar_id,
            events,
        )
    }

    pub fn add_scheme_to_folder(
        &mut self,
        key: DeviceKey,
        folder: FolderId,
        name: &str,
        lines: &[&str],
    ) -> SchemeId {
        self.device_mut(key)
            .add_scheme_to_folder(folder, name, lines)
    }

    pub fn add_subfolder(&mut self, key: DeviceKey, parent: FolderId, name: &str) -> FolderId {
        self.device_mut(key).add_subfolder(parent, name)
    }

    pub fn archive_folder(&mut self, key: DeviceKey, folder: FolderId) {
        self.device_mut(key).archive_folder(folder);
    }

    pub fn restore_folder(&mut self, key: DeviceKey, folder: FolderId) {
        self.device_mut(key).restore_folder(folder);
    }

    /// Archive a scheme, then permanently remove it from the workspace and scheme_sync
    /// index (mirrors `PermanentlyDeleteScheme` in the desktop commands crate).
    /// After this call the scheme's content doc lingers server-side; other devices
    /// that pull it get a benign `unknown_scheme_document` skip.
    pub fn delete_scheme(&mut self, key: DeviceKey, scheme: SchemeId) {
        self.device_mut(key).delete_scheme(scheme);
    }

    /// Archive a folder, then permanently remove it and its subtree from the workspace
    /// (mirrors `PermanentlyDeleteFolder` in the desktop commands crate).
    pub fn delete_folder(&mut self, key: DeviceKey, folder: FolderId) {
        self.device_mut(key).delete_folder(folder);
    }

    /// Rename a folder.
    pub fn rename_folder(&mut self, key: DeviceKey, folder: FolderId, name: &str) {
        self.device_mut(key).rename_folder(folder, name);
    }

    /// Move a scheme back to the root folder from wherever it currently lives.
    pub fn move_scheme_to_root(&mut self, key: DeviceKey, scheme: SchemeId) {
        self.device_mut(key).move_scheme_to_root(scheme);
    }

    /// Change the marker (blank/bullet/numbered/checkbox) on an item.
    pub fn set_item_marker(
        &mut self,
        key: DeviceKey,
        scheme: SchemeId,
        item_index: usize,
        marker: ItemMarker,
    ) {
        self.device_mut(key)
            .set_item_marker(scheme, item_index, marker);
    }

    /// Set start and/or end dates on an item (checkbox marker applied automatically).
    pub fn set_item_dates(
        &mut self,
        key: DeviceKey,
        scheme: SchemeId,
        item_index: usize,
        start: Option<chrono::DateTime<chrono::Utc>>,
        end: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        self.device_mut(key)
            .set_item_dates(scheme, item_index, start, end);
    }

    /// Change the indent level on an item.
    pub fn set_item_indent(
        &mut self,
        key: DeviceKey,
        scheme: SchemeId,
        item_index: usize,
        indent: u8,
    ) {
        self.device_mut(key)
            .set_item_indent(scheme, item_index, indent);
    }

    /// Push a notification schedule change from device `key`.  Returns the
    /// `notification_schedule_revision` reported by the server.
    ///
    /// Over the in-memory TestServer this is always 0 (the test server does not
    /// track notification schedule revisions).  Over HTTP, the real backend returns
    /// a monotonically increasing revision.
    pub fn update_notification_schedule(
        &mut self,
        key: DeviceKey,
        sequence: u64,
        hash: &str,
    ) -> u64 {
        let mut device = self.devices.remove(&key).expect("missing device");
        let rev = match &self.backend {
            HarnessBackend::InMemory(server) => {
                device.update_notification_schedule_with(server, sequence, hash)
            }
            HarnessBackend::Http(clients) => {
                let client = clients
                    .get(&key)
                    .unwrap_or_else(|| panic!("no HTTP client for {key:?}"));
                device.update_notification_schedule_with(client, sequence, hash)
            }
        };
        self.devices.insert(key, device);
        rev
    }

    pub fn set_daily_queue(
        &mut self,
        key: DeviceKey,
        date: chrono::NaiveDate,
        lines: &[&str],
    ) -> SchemeId {
        self.device_mut(key).set_daily_queue(date, lines)
    }

    /// Direct (non-command) Daily Queue creation that leaves the scheme's CRDT
    /// document empty — see [`TestDevice::set_daily_queue_without_crdt_content`].
    pub fn set_daily_queue_without_crdt_content(
        &mut self,
        key: DeviceKey,
        date: chrono::NaiveDate,
        lines: &[&str],
    ) -> SchemeId {
        self.device_mut(key)
            .set_daily_queue_without_crdt_content(date, lines)
    }

    /// Faithful daily-queue creation with rich rows (dates/done/markers) — see
    /// [`TestDevice::seed_daily_queue`].
    pub fn seed_daily_queue(
        &mut self,
        key: DeviceKey,
        date: chrono::NaiveDate,
        items: Vec<Item>,
    ) -> SchemeId {
        self.device_mut(key).seed_daily_queue(date, items)
    }

    /// Roll today's blank daily queue over from the most recent non-blank prior day —
    /// see [`TestDevice::carryover_daily_queue`]. Returns the carried row texts.
    pub fn carryover_daily_queue(
        &mut self,
        key: DeviceKey,
        today: chrono::NaiveDate,
    ) -> Option<Vec<String>> {
        self.device_mut(key).carryover_daily_queue(today)
    }

    /// Record a workspace-index change on a device (e.g. the `daily_queue` map
    /// entry a direct Daily Queue creation adds).
    pub fn record_workspace_change_pub(&mut self, key: DeviceKey) {
        self.device_mut(key)
            .record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    pub fn sync(&mut self, key: DeviceKey) {
        self.try_sync(key).expect("sync");
    }

    /// Convenience: `remote_latest_after_sync` for a device (useful for media upload).
    pub fn device_remote_latest(&self, key: DeviceKey) -> HashMap<DocumentId, u64> {
        self.device(key).remote_latest_after_sync()
    }

    /// Record a scheme content change on a device — used by scenarios that directly
    /// mutate scheme items (e.g. gsync re-import simulation).
    pub fn record_scheme_change_pub(&mut self, key: DeviceKey, scheme: SchemeId) {
        self.device_mut(key)
            .record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme));
    }

    /// Like [`sync`] but returns the push result rather than panicking on failure.
    pub fn try_sync(&mut self, key: DeviceKey) -> anyhow::Result<()> {
        let mut device = self.devices.remove(&key).expect("missing device");
        let result = match &self.backend {
            HarnessBackend::InMemory(server) => device.try_sync(server),
            HarnessBackend::Http(clients) => {
                let client = clients
                    .get(&key)
                    .unwrap_or_else(|| panic!("no HTTP client for {key:?}"));
                device.try_sync_with(client)
            }
        };
        self.devices.insert(key, device);
        result
    }

    /// Restart the device at `key` using the **legacy** (buggy) `next_sequence = 1`
    /// behavior, faithfully reproducing today's desktop restart semantics.
    pub fn restart_legacy(&mut self, key: DeviceKey) {
        self.device_mut(key).restart_legacy_sequence_reset();
    }

    /// Restart the device at `key` using the **fixed** behavior: `next_sequence` is
    /// seeded from the highest already-used sequence + 1.
    pub fn restart(&mut self, key: DeviceKey) {
        self.device_mut(key).restart();
    }

    /// Arm a one-shot `crdt_schema_invalid` rejection on the test server so the
    /// next push call deterministically exercises the engine's self-heal path.
    /// Panics on an HTTP harness (fault injection is only available in-memory).
    pub fn reject_next_push_with_schema_invalid(&self) {
        self.require_in_memory_server()
            .reject_next_push_with_schema_invalid();
    }

    /// Arm a one-shot rejection with an arbitrary backend code so the next push
    /// exercises the engine's generalized (non-`crdt_schema_invalid`) self-heal.
    /// Panics on an HTTP harness (fault injection is only available in-memory).
    pub fn reject_next_push_with_code(&self, code: &str) {
        self.require_in_memory_server()
            .reject_next_push_with_code(code);
    }

    /// In-memory only.
    pub fn server_pull_calls(&self) -> usize {
        self.require_in_memory_server().pull_calls()
    }

    /// In-memory only.
    pub fn server_push_calls(&self) -> usize {
        self.require_in_memory_server().push_calls()
    }

    /// In-memory only.
    pub fn server_document_count(&self) -> usize {
        self.require_in_memory_server().document_count()
    }

    /// In-memory only.
    pub fn server_media_asset_count(&self) -> usize {
        self.require_in_memory_server().media_asset_count()
    }

    /// Attach a synthetic PNG image to item `item_index` of `scheme` on device `key`.
    /// Returns `(asset Uuid, image_name)`.
    pub fn attach_image_to_device(
        &mut self,
        key: DeviceKey,
        scheme: SchemeId,
        item_index: usize,
        bytes: Vec<u8>,
    ) -> (uuid::Uuid, String) {
        self.device_mut(key).attach_image(scheme, item_index, bytes)
    }

    /// Upload all pending media assets from device `key` to the server (in-memory or HTTP).
    pub fn upload_media(
        &mut self,
        key: DeviceKey,
        remote_latest: &HashMap<DocumentId, u64>,
    ) -> anyhow::Result<()> {
        let mut device = self.devices.remove(&key).expect("missing device");
        let result = match &self.backend {
            HarnessBackend::InMemory(server) => device.upload_media_to(server, remote_latest),
            HarnessBackend::Http(clients) => {
                let client = clients
                    .get(&key)
                    .unwrap_or_else(|| panic!("no HTTP client for {key:?}"));
                device.upload_media_to_http(client, remote_latest)
            }
        };
        self.devices.insert(key, device);
        result
    }

    /// Download all missing media assets for device `key` from the server (in-memory or HTTP).
    pub fn download_media(&mut self, key: DeviceKey) {
        let mut device = self.devices.remove(&key).expect("missing device");
        match &self.backend {
            HarnessBackend::InMemory(server) => device.download_media_from(server),
            HarnessBackend::Http(clients) => {
                let client = clients
                    .get(&key)
                    .unwrap_or_else(|| panic!("no HTTP client for {key:?}"));
                device
                    .download_media_from_http(client)
                    .expect("download_media_from_http");
            }
        }
        self.devices.insert(key, device);
    }

    /// Inject an orphan scheme content document on the server (no workspace-index
    /// entry).  Returns the `DocumentId` that was injected.
    /// In-memory only — panics on HTTP harness.
    pub fn inject_orphan_scheme_document(&self, scheme: &Scheme) -> DocumentId {
        self.require_in_memory_server()
            .inject_orphan_scheme_document(scheme)
    }

    /// Corrupt the personal workspace document on the server for device `key`.
    /// In-memory only — panics on HTTP harness.
    pub fn corrupt_workspace_document(&self, key: DeviceKey) {
        let workspace_doc_id = self.device(key).workspace.sync.id;
        self.require_in_memory_server()
            .corrupt_workspace_document(workspace_doc_id);
    }

    /// In-memory only — panics on HTTP harness.
    pub fn push_remote_workspace_snapshot(&self, workspace: &Workspace) {
        let server = self.require_in_memory_server();
        let documents = WorkspaceCrdtDocuments::snapshot_updates(workspace)
            .updates
            .into_iter()
            .map(|update| PushDocumentUpdates {
                document: update.document,
                kind: update.kind,
                updates: vec![update.update_v1],
            })
            .collect::<Vec<_>>();
        server
            .push(&BatchPushRequest {
                replica_id: ReplicaId::new(),
                documents,
                notification_schedule_changed: false,
                notification_schedule: Some(test_notification_schedule()),
            })
            .expect("push remote workspace snapshot");
    }

    /// Sync every device until all replicas reach a fixed point (byte-identical
    /// summaries) or a generous round budget is exhausted.
    pub fn settle(&mut self) {
        let keys = self.device_keys();
        let max_rounds = keys.len() * 4 + 8;
        for _ in 0..max_rounds {
            for key in &keys {
                self.sync(*key);
            }
            if self.all_summaries_equal() {
                for key in &keys {
                    self.sync(*key);
                }
                if self.all_summaries_equal() {
                    return;
                }
            }
        }
    }

    fn all_summaries_equal(&self) -> bool {
        let mut iter = self.devices.values();
        let Some(first) = iter.next() else {
            return true;
        };
        let expected = first.summary();
        iter.all(|device| device.summary() == expected)
    }

    // --- assertions ---

    pub fn assert_all_converged(&self) {
        self.assert_all_converged_with_context(0);
    }

    pub fn assert_all_converged_with_context(&self, seed: u64) {
        let keys = self.device_keys();
        let Some((first, rest)) = keys.split_first() else {
            return;
        };
        let expected = self.device(*first).summary();
        for key in rest {
            let actual = self.device(*key).summary();
            assert_eq!(
                actual, expected,
                "seed {seed}: {key:?} diverged from {first:?}"
            );
        }
    }

    /// Assert a scheme has been permanently deleted: it is absent from both
    /// `workspace.schemes` AND the workspace tree.
    pub fn assert_scheme_absent(&self, key: DeviceKey, scheme: SchemeId) {
        let device = self.device(key);
        assert!(
            !device.workspace.schemes.contains_key(&scheme),
            "{key:?}: scheme {scheme} still present in workspace.schemes after delete"
        );
    }

    pub fn assert_scheme_active(&self, key: DeviceKey, scheme: SchemeId) {
        let device = self.device(key);
        assert!(
            !device.workspace.is_scheme_deleted(scheme),
            "{scheme} archived"
        );
        assert!(
            device.root_scheme_ids().contains(&scheme),
            "{scheme} missing from root"
        );
    }

    pub fn assert_scheme_archived(&self, key: DeviceKey, scheme: SchemeId) {
        let device = self.device(key);
        assert!(
            device.workspace.is_scheme_deleted(scheme),
            "{scheme} not archived"
        );
        assert!(
            !device.root_scheme_ids().contains(&scheme),
            "{scheme} reintroduced into root"
        );
    }

    pub fn assert_scheme_items(&self, key: DeviceKey, scheme: SchemeId, expected: &[&str]) {
        let actual = self.device(key).scheme_item_texts(scheme);
        let expected = expected.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    pub fn assert_scheme_items_unordered(
        &self,
        key: DeviceKey,
        scheme: SchemeId,
        expected: &[&str],
    ) {
        let mut actual = self.device(key).scheme_item_texts(scheme);
        let mut expected = expected.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
    }

    pub fn assert_scheme_name(&self, key: DeviceKey, scheme: SchemeId, expected: &str) {
        let actual = self
            .device(key)
            .workspace
            .schemes
            .get(&scheme)
            .map(|scheme| scheme.name.clone());
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "{key:?} scheme {scheme} name mismatch",
        );
    }

    /// The scheme's imported-calendar source as materialized on `key`, if any.
    pub fn imported_calendar_source(
        &self,
        key: DeviceKey,
        scheme: SchemeId,
    ) -> Option<ImportedCalendarSource> {
        match self
            .device(key)
            .workspace
            .schemes
            .get(&scheme)?
            .source
            .clone()
        {
            SchemeSource::ImportedCalendar(source) => Some(source),
            SchemeSource::Local => None,
        }
    }

    /// A deleted folder must survive in the archive *as a folder*: it stays in the
    /// folders map, is out of the sidebar tree, and still nests its scheme children.
    pub fn assert_archived_folder_with_schemes(
        &self,
        key: DeviceKey,
        folder: FolderId,
        schemes: &[SchemeId],
    ) {
        let device = self.device(key);
        let archived = device
            .workspace
            .folders
            .get(&folder)
            .unwrap_or_else(|| panic!("{key:?}: archived folder {folder} vanished entirely"));
        let children: Vec<SchemeId> = archived
            .children
            .iter()
            .filter_map(|child| match child {
                NodeRef::Scheme(id) => Some(*id),
                NodeRef::Folder(_) => None,
            })
            .collect();
        for scheme in schemes {
            assert!(
                children.contains(scheme),
                "{key:?}: archived folder {folder} lost scheme {scheme} (flattened?)",
            );
        }
        assert!(
            !device
                .root_scheme_ids()
                .iter()
                .any(|id| schemes.contains(id)),
            "{key:?}: archived folder's schemes leaked back into the sidebar root",
        );
    }
}

// ---------------------------------------------------------------------------
// Test server — implements the real SyncTransport against the merged-state model
// ---------------------------------------------------------------------------

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

impl TestDevice {
    /// Construct a `TestDevice` from a `Workspace` and canonical `account_workspace` id.
    /// Used by integration tests that need to share the backend's provisioned workspace_id.
    pub fn new_from_base(base: &Workspace, account_workspace: WorkspaceId) -> Self {
        Self::from_base(base, account_workspace)
    }

    fn from_base(base: &Workspace, account_workspace: WorkspaceId) -> Self {
        let mut workspace = base.clone();
        workspace.canonicalize_personal_sync_identity(account_workspace);
        let replica_id = ReplicaId::new();
        let store_crdt =
            WorkspaceCrdtDocuments::from_states(&workspace, replica_id, &HashMap::new())
                .expect("seed store crdt");
        let crdt_states = store_crdt.document_states();
        let local_state = LocalSyncState {
            workspace_id: Some(workspace.id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        Self {
            account_workspace,
            replica_id,
            workspace,
            store_crdt,
            crdt_states,
            local_state,
            next_sequence: 1,
            media_assets: HashMap::new(),
            last_skipped: Vec::new(),
        }
    }

    // --- restart simulation ----------------------------------------------------

    /// Simulate an app restart with the **buggy** desktop behavior
    /// (`desktop/state/src/store.rs:87`): `next_sequence` is hard-coded to 1
    /// regardless of what's already in `local_state.pending`.  The `store_crdt` is
    /// rebuilt from the persisted `crdt_states` (the on-disk `sync-crdt-state.json`)
    /// and `local_state` is kept as-is (the on-disk `sync-state.json`), faithfully
    /// replicating what the desktop does today.
    ///
    /// After this call, new edits will reuse local_sequence 1, 2, 3 … even if older
    /// unpushed edits with those sequences are still in `pending`.
    pub fn restart_legacy_sequence_reset(&mut self) {
        // Rebuild store CRDT from persisted state (stable deterministic clientID).
        self.store_crdt = WorkspaceCrdtDocuments::from_states(
            &self.workspace,
            self.replica_id,
            &self.crdt_states,
        )
        .expect("restart: rebuild store_crdt from crdt_states");
        // Reset next_sequence to 1 — this is the bug we're reproducing.
        self.next_sequence = 1;
        // local_state (pending edits + cursors) is kept exactly as-is, mirroring
        // how the desktop reads sync-state.json from disk at startup.
    }

    /// Simulate an app restart with the **correct** behavior: `next_sequence` is
    /// seeded from `max(pending local_sequence, document_cursors last_pushed_sequence)
    /// + 1`, so new edits never reuse a sequence number that's already in `pending`
    /// or was already pushed.  This is what mobile already does
    /// (`mobile/core/src/lib.rs:820-841`).
    pub fn restart(&mut self) {
        // Rebuild store CRDT from persisted state (same as legacy path).
        self.store_crdt = WorkspaceCrdtDocuments::from_states(
            &self.workspace,
            self.replica_id,
            &self.crdt_states,
        )
        .expect("restart: rebuild store_crdt from crdt_states");
        // Seed next_sequence from the highest sequence number already in use —
        // either still pending or already acknowledged — plus one.
        let max_pending = self
            .local_state
            .pending
            .iter()
            .map(|edit| edit.local_sequence)
            .max()
            .unwrap_or(0);
        let max_pushed = self
            .local_state
            .document_cursors
            .values()
            .map(|cursor| cursor.last_pushed_sequence)
            .max()
            .unwrap_or(0);
        self.next_sequence = max_pending.max(max_pushed) + 1;
        // local_state is kept as-is.
    }

    // --- pending-queue inspection ----------------------------------------------

    /// Number of edits currently in the outbound pending queue.
    pub fn pending_count(&self) -> usize {
        self.local_state.pending.len()
    }

    /// `true` when the pending queue is empty.
    pub fn is_fully_pushed(&self) -> bool {
        self.local_state.pending.is_empty()
    }

    /// Return a snapshot of the pending edits (cloned) for inspection.
    pub fn pending_edits(&self) -> Vec<knotq_sync::PendingCrdtEdit> {
        self.local_state.pending.iter().cloned().collect()
    }

    /// Direct mutable access to `local_state` for white-box surgery in regression
    /// tests (e.g. simulating a partially-acked push that left stale entries).
    pub fn local_state_mut(&mut self) -> &mut LocalSyncState {
        &mut self.local_state
    }

    // --- media helpers ---------------------------------------------------------

    /// Attach a synthetic PNG image to item `item_index` in `scheme_id`, writing
    /// `bytes` into the device's in-memory media assets map (stand-in for the
    /// on-disk `media/` dir), and adding an `ItemMedia::Image` entry to the item.
    /// Also records a CRDT change so the edit is pushed to the server.
    pub fn attach_image(
        &mut self,
        scheme_id: SchemeId,
        item_index: usize,
        bytes: Vec<u8>,
    ) -> (Uuid, String) {
        let asset = Uuid::new_v4();
        let format = ImageAssetFormat::Png;
        let image_name = format!("{}.{}", asset, format.extension());
        {
            let items = &mut self.scheme_mut(scheme_id).items;
            if item_index < items.len() {
                items[item_index].set_image(ImageInline {
                    asset,
                    format,
                    width: Some(64),
                    height: Some(64),
                });
            }
        }
        self.media_assets.insert(image_name.clone(), bytes);
        self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
        (asset, image_name)
    }

    /// Upload all locally-held media assets that `should_upload_media_asset` says
    /// need uploading (mirrors `upload_local_media_assets` in `sync_service.rs`).
    pub fn upload_media_to(
        &mut self,
        server: &TestServer,
        remote_latest: &HashMap<DocumentId, u64>,
    ) -> anyhow::Result<()> {
        use sha2::{Digest, Sha256};

        // Phase 1: collect (document, image_name) refs from the workspace without
        // borrowing self.media_assets or self.local_state.
        let refs: Vec<(DocumentId, String)> = self
            .workspace
            .schemes
            .iter()
            .filter_map(|(scheme_id, scheme)| {
                let meta = self.workspace.scheme_sync.get(scheme_id)?;
                Some((meta.id, scheme))
            })
            .flat_map(|(document, scheme)| {
                scheme.items.iter().flat_map(move |item| {
                    item_image_assets(item).into_iter().map(move |media| {
                        let image_name = format!("{}.{}", media.asset, media.format.extension());
                        (document, image_name)
                    })
                })
            })
            .collect();

        // Phase 2: for each ref, check cursor + upload if needed.
        for (document, image_name) in refs {
            let Some(bytes) = self.media_assets.get(&image_name).cloned() else {
                continue; // asset not on disk — skip (mirror of the real driver)
            };
            if bytes.is_empty() {
                continue;
            }
            let byte_length = bytes.len() as u64;
            let digest = Sha256::digest(&bytes);
            let sha256: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            if !self.local_state.should_upload_media_asset(
                &image_name,
                document,
                byte_length,
                &sha256,
                remote_latest,
            ) {
                continue;
            }
            server.upload_media(document, &image_name, bytes)?;
            self.local_state
                .mark_media_uploaded(image_name, document, byte_length, sha256);
        }
        Ok(())
    }

    /// Download any media assets referenced by workspace items that aren't already
    /// in `self.media_assets` (mirrors `download_missing_media_assets`).
    pub fn download_media_from(&mut self, server: &TestServer) {
        // Collect (document, image_name) refs first to avoid holding a borrow on
        // self.workspace while mutating self.media_assets.
        let refs: Vec<(DocumentId, String)> = self
            .workspace
            .schemes
            .keys()
            .filter_map(|scheme_id| {
                let meta = self.workspace.scheme_sync.get(scheme_id)?;
                let scheme = self.workspace.schemes.get(scheme_id)?;
                Some((meta.id, scheme.items.clone()))
            })
            .flat_map(|(document, items)| {
                items.into_iter().flat_map(move |item| {
                    item_image_assets(&item).into_iter().map({
                        move |media| {
                            let image_name =
                                format!("{}.{}", media.asset, media.format.extension());
                            (document, image_name)
                        }
                    })
                })
            })
            .collect();

        for (document, image_name) in refs {
            if self.media_assets.contains_key(&image_name) {
                continue; // already present
            }
            if let Some(bytes) = server.download_media(document, &image_name) {
                self.media_assets.insert(image_name, bytes);
            }
        }
    }

    // ---------------------------------------------------------------------------

    pub fn add_scheme(&mut self, name: &str, lines: &[&str]) -> SchemeId {
        let mut scheme = Scheme::new(name, 0);
        for line in lines {
            scheme.items.push(Item::new(*line));
        }
        let scheme_id = scheme.id;
        self.workspace
            .folders
            .get_mut(&self.workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));
        self.workspace.schemes.insert(scheme_id, scheme);
        self.record_changes(
            WorkspaceCrdtChangeSet::default()
                .workspace()
                .touch_scheme(scheme_id),
        );
        scheme_id
    }

    pub fn append_line(&mut self, scheme_id: SchemeId, text: &str) {
        self.scheme_mut(scheme_id).items.push(Item::new(text));
        self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
    }

    pub fn edit_line(&mut self, scheme_id: SchemeId, index: usize, text: &str) {
        let items = &mut self.scheme_mut(scheme_id).items;
        if index < items.len() {
            items[index].set_text(text);
            self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
        }
    }

    pub fn insert_line(&mut self, scheme_id: SchemeId, index: usize, text: &str) {
        let items = &mut self.scheme_mut(scheme_id).items;
        let index = index.min(items.len());
        items.insert(index, Item::new(text));
        self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
    }

    pub fn remove_line(&mut self, scheme_id: SchemeId, index: usize) {
        let items = &mut self.scheme_mut(scheme_id).items;
        if index < items.len() {
            items.remove(index);
            self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
        }
    }

    pub fn reorder_reverse(&mut self, scheme_id: SchemeId) {
        self.scheme_mut(scheme_id).items.reverse();
        self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
    }

    pub fn rename_scheme(&mut self, scheme_id: SchemeId, name: &str) {
        self.scheme_mut(scheme_id).name = name.to_string();
        // The name lives in the workspace document's node payload.
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    pub fn add_folder(&mut self, name: &str) -> FolderId {
        let folder = Folder {
            id: FolderId::new(),
            name: name.to_string(),
            parent: Some(self.workspace.root),
            children: Vec::new(),
            expanded: true,
        };
        let id = folder.id;
        self.workspace
            .folders
            .get_mut(&self.workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(id));
        self.workspace.folders.insert(id, folder);
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
        id
    }

    pub fn move_scheme_to_folder(&mut self, scheme_id: SchemeId, folder_id: FolderId) {
        let root = self.workspace.root;
        self.workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .retain(|child| *child != NodeRef::Scheme(scheme_id));
        if let Some(folder) = self.workspace.folders.get_mut(&folder_id) {
            folder.children.push(NodeRef::Scheme(scheme_id));
        }
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    pub fn archive_scheme(&mut self, scheme_id: SchemeId) {
        let root = self.workspace.root;
        let position = self
            .workspace
            .folders
            .get(&root)
            .and_then(|folder| {
                folder
                    .children
                    .iter()
                    .position(|child| *child == NodeRef::Scheme(scheme_id))
            })
            .unwrap_or(0);
        for folder in self.workspace.folders.values_mut() {
            folder
                .children
                .retain(|child| *child != NodeRef::Scheme(scheme_id));
        }
        self.workspace
            .mark_scheme_deleted_from(scheme_id, root, position);
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    pub fn restore_scheme(&mut self, scheme_id: SchemeId) {
        self.workspace.unmark_scheme_deleted(scheme_id);
        let root = self.workspace.root;
        let already_present = self
            .workspace
            .folders
            .values()
            .any(|folder| folder.children.contains(&NodeRef::Scheme(scheme_id)));
        if !already_present {
            self.workspace
                .folders
                .get_mut(&root)
                .unwrap()
                .children
                .push(NodeRef::Scheme(scheme_id));
        }
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    pub fn import_calendar_scheme(
        &mut self,
        name: &str,
        account_id: &str,
        account_email: &str,
        calendar_id: &str,
        events: &[&str],
    ) -> SchemeId {
        let mut scheme = Scheme::new(name, 0);
        scheme.gsync = true;
        scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: account_id.to_string(),
            account_email: Some(account_email.to_string()),
            calendar_id: calendar_id.to_string(),
            sync_token: Some("local-only-sync-token".to_string()),
            read_only: true,
            last_synced_at: None,
        });
        for event in events {
            scheme.items.push(Item::new(*event));
        }
        let scheme_id = scheme.id;
        self.workspace
            .folders
            .get_mut(&self.workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));
        self.workspace.schemes.insert(scheme_id, scheme);
        self.record_changes(
            WorkspaceCrdtChangeSet::default()
                .workspace()
                .touch_scheme(scheme_id),
        );
        scheme_id
    }

    pub fn add_scheme_to_folder(
        &mut self,
        folder_id: FolderId,
        name: &str,
        lines: &[&str],
    ) -> SchemeId {
        let mut scheme = Scheme::new(name, 0);
        for line in lines {
            scheme.items.push(Item::new(*line));
        }
        let scheme_id = scheme.id;
        self.workspace
            .folders
            .get_mut(&folder_id)
            .expect("unknown folder")
            .children
            .push(NodeRef::Scheme(scheme_id));
        self.workspace.schemes.insert(scheme_id, scheme);
        self.record_changes(
            WorkspaceCrdtChangeSet::default()
                .workspace()
                .touch_scheme(scheme_id),
        );
        scheme_id
    }

    pub fn add_subfolder(&mut self, parent: FolderId, name: &str) -> FolderId {
        let folder = Folder {
            id: FolderId::new(),
            name: name.to_string(),
            parent: Some(parent),
            children: Vec::new(),
            expanded: true,
        };
        let id = folder.id;
        self.workspace
            .folders
            .get_mut(&parent)
            .expect("unknown parent folder")
            .children
            .push(NodeRef::Folder(id));
        self.workspace.folders.insert(id, folder);
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
        id
    }

    /// Delete a folder as one archive unit: the top folder is detached from the
    /// sidebar tree, but the folder subtree remains intact in the workspace maps.
    pub fn archive_folder(&mut self, folder_id: FolderId) {
        let parent = self
            .workspace
            .folders
            .get(&folder_id)
            .and_then(|folder| folder.parent);
        let Some(parent) = parent else {
            return;
        };
        let Some(position) = self.workspace.folders.get(&parent).and_then(|folder| {
            folder
                .children
                .iter()
                .position(|child| *child == NodeRef::Folder(folder_id))
        }) else {
            return;
        };
        if let Some(folder) = self.workspace.folders.get_mut(&parent) {
            folder
                .children
                .retain(|child| *child != NodeRef::Folder(folder_id));
        }
        self.workspace
            .mark_folder_deleted_from(folder_id, parent, position);
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    pub fn restore_folder(&mut self, folder_id: FolderId) {
        // Mirrors restoring a folder unit: re-home it (and any surviving subtree)
        // under root and clear archival on its schemes.
        let root = self.workspace.root;
        self.workspace
            .folders
            .entry(folder_id)
            .or_insert_with(|| Folder {
                id: folder_id,
                name: "Restored".to_string(),
                parent: Some(root),
                children: Vec::new(),
                expanded: true,
            });
        let already_present = self
            .workspace
            .folders
            .values()
            .any(|folder| folder.children.contains(&NodeRef::Folder(folder_id)));
        if !already_present {
            self.workspace
                .folders
                .get_mut(&root)
                .unwrap()
                .children
                .push(NodeRef::Folder(folder_id));
        }
        self.workspace.unmark_folder_deleted(folder_id);
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    pub fn set_daily_queue(&mut self, date: chrono::NaiveDate, lines: &[&str]) -> SchemeId {
        let daily_id = daily_queue_scheme_id(date);
        let mut scheme = Scheme::new("Daily", 0);
        scheme.id = daily_id;
        for line in lines {
            scheme.items.push(Item::new(*line));
        }
        self.workspace.schemes.insert(daily_id, scheme);
        self.workspace.daily_queue.insert(date, daily_id);
        self.record_changes(
            WorkspaceCrdtChangeSet::default()
                .workspace()
                .touch_scheme(daily_id),
        );
        daily_id
    }

    /// Simulate the desktop's direct (non-command) Daily Queue creation as the
    /// store behaved before it recorded direct CRDT changes: the scheme is
    /// inserted into the workspace and the store CRDT is rebuilt from the
    /// persisted states (mirroring `WorkspaceStore::replace_workspace`), leaving
    /// the new scheme's CRDT document EMPTY — no `schema` root, no items. This
    /// is the on-disk state that wedged production pushes with
    /// `crdt_schema_invalid` on 2026-06-11.
    pub fn set_daily_queue_without_crdt_content(
        &mut self,
        date: chrono::NaiveDate,
        lines: &[&str],
    ) -> SchemeId {
        let daily_id = daily_queue_scheme_id(date);
        let mut scheme = Scheme::new("Daily", 0);
        scheme.id = daily_id;
        for line in lines {
            scheme.items.push(Item::new(*line));
        }
        self.workspace.schemes.insert(daily_id, scheme);
        self.workspace.daily_queue.insert(date, daily_id);
        self.workspace
            .canonicalize_personal_sync_identity(self.account_workspace);
        self.workspace.ensure_sync_metadata();
        self.store_crdt = WorkspaceCrdtDocuments::from_states(
            &self.workspace,
            self.replica_id,
            &self.crdt_states,
        )
        .expect("rebuild store crdt");
        self.crdt_states = self.store_crdt.document_states();
        daily_id
    }

    /// Faithful daily-queue creation that accepts pre-built rich rows (dates, done
    /// state, markers) rather than plain text. Uses the deterministic daily SchemeId
    /// and lets `ensure_sync_metadata` canonicalize the deterministic daily DocumentId,
    /// so two devices that create the same day independently converge on one document.
    /// Mirrors `App::ensure_daily_queue_scheme` plus direct row edits.
    pub fn seed_daily_queue(&mut self, date: NaiveDate, items: Vec<Item>) -> SchemeId {
        let daily_id = daily_queue_scheme_id(date);
        // Match `set_daily_queue`'s name so the two helpers are interchangeable for the
        // same date across devices (the convergence check compares scheme names).
        let mut scheme = Scheme::new("Daily", 0);
        scheme.id = daily_id;
        scheme.items = items;
        self.workspace.schemes.insert(daily_id, scheme);
        self.workspace.daily_queue.insert(date, daily_id);
        self.record_changes(
            WorkspaceCrdtChangeSet::default()
                .workspace()
                .touch_scheme(daily_id),
        );
        daily_id
    }

    /// Mirror of the desktop "roll over from yesterday" action — the net effect of
    /// `knotq_state::daily_queue_carryover_command` applied via
    /// `App::carryover_daily_queue`. Every not-fully-complete row from the most recent
    /// non-blank prior day (within the 14-day lookback) is cloned forward into `today`
    /// with a FRESH `ItemId`; the source rows keep their text but have their date
    /// annotations stripped; and today's blank placeholder is replaced by the first
    /// carried row. The action touches BOTH the previous and today scheme documents in
    /// one logical batch — the cross-document property that makes it a hard sync case.
    ///
    /// `today`'s scheme must already exist (the scenario creates it, as the real app's
    /// `ensure_daily_queue_scheme` does before carrying over). Returns the carried row
    /// texts, or `None` when there is nothing to carry.
    pub fn carryover_daily_queue(&mut self, today: NaiveDate) -> Option<Vec<String>> {
        let previous_date = dq_last_nonblank_day(&self.workspace, today)?;
        let previous_id = self.workspace.daily_queue_scheme_id(previous_date)?;
        let today_id = self.workspace.daily_queue_scheme_id(today)?;

        // Build the carried rows (fresh ids) and the list of source rows to strip,
        // from an immutable borrow of the previous scheme.
        let (carried_items, strip_ids): (Vec<Item>, Vec<ItemId>) = {
            let previous = self.workspace.scheme(previous_id)?;
            if dq_scheme_is_blank(previous) {
                return None;
            }
            let mut carried = Vec::new();
            let mut strip = Vec::new();
            for item in &previous.items {
                if dq_item_is_fully_complete_task(item) {
                    continue;
                }
                let mut clone = item.clone();
                clone.id = ItemId::new();
                carried.push(clone);
                if dq_item_has_annotations(item) {
                    strip.push(item.id);
                }
            }
            (carried, strip)
        };
        if carried_items.is_empty() {
            return None;
        }
        let carried_texts: Vec<String> = carried_items.iter().map(|i| i.text()).collect();

        // Strip date annotations from the source rows on the previous day.
        {
            let previous = self.scheme_mut(previous_id);
            for item in previous.items.iter_mut() {
                if strip_ids.contains(&item.id) {
                    dq_strip_annotations(item);
                }
            }
        }

        // Insert the carried rows into today, replacing the blank placeholder with the
        // first carried row (the `daily_queue_carryover_command` placeholder branch).
        {
            let today_scheme = self.scheme_mut(today_id);
            let replace_placeholder =
                dq_scheme_is_blank(today_scheme) && !today_scheme.items.is_empty();
            let mut position = today_scheme.items.len();
            let mut carried = carried_items.into_iter();
            if replace_placeholder {
                if let Some(mut first) = carried.next() {
                    first.id = today_scheme.items[0].id;
                    today_scheme.items[0] = first;
                }
                position = 1;
            }
            for item in carried {
                let at = position.min(today_scheme.items.len());
                today_scheme.items.insert(at, item);
                position += 1;
            }
        }

        self.record_changes(
            WorkspaceCrdtChangeSet::default()
                .touch_scheme(previous_id)
                .touch_scheme(today_id),
        );
        Some(carried_texts)
    }

    /// The sync document id backing `scheme_id`.
    pub fn scheme_document_id(&self, scheme_id: SchemeId) -> DocumentId {
        self.workspace
            .scheme_sync
            .get(&scheme_id)
            .expect("scheme sync metadata")
            .id
    }

    /// Queue a raw pending edit, bypassing the CRDT — test surgery for
    /// reproducing exact on-disk pending-queue states (e.g. the 2-byte empty
    /// Yjs update a schema-less document snapshot produces).
    pub fn push_raw_pending_edit(
        &mut self,
        document: DocumentId,
        kind: SyncDocumentKind,
        update_v1: Vec<u8>,
    ) {
        let local_sequence = self.next_sequence;
        self.next_sequence += 1;
        self.local_state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: self.workspace.id,
            replica_id: self.replica_id,
            local_sequence,
            created_at: Utc::now(),
            document,
            kind,
            update_v1,
        });
    }

    /// Archive and then permanently delete a scheme, mirroring
    /// `PermanentlyDeleteScheme`.  After this call, `workspace.schemes` no longer
    /// contains the scheme, and `ensure_sync_metadata` will drop its `scheme_sync`
    /// entry, so the next workspace push removes it from the server's workspace index.
    /// The content document lingers server-side; other devices that pull it receive a
    /// benign `unknown_scheme_document` skip.
    pub fn delete_scheme(&mut self, scheme_id: SchemeId) {
        // Step 1: remove from all folder children lists.
        for folder in self.workspace.folders.values_mut() {
            folder
                .children
                .retain(|child| *child != NodeRef::Scheme(scheme_id));
        }
        // Step 2: remove from recently_deleted (archive state) if present.
        self.workspace
            .recently_deleted
            .retain(|id| *id != scheme_id);
        self.workspace.deleted_scheme_origins.remove(&scheme_id);
        // Step 3: remove the scheme itself — this triggers scheme_sync cleanup in
        // ensure_sync_metadata on the next sync.
        self.workspace.schemes.remove(&scheme_id);
        // Step 4: record as workspace change so the deletion propagates via CRDT.
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    /// Archive and permanently delete a folder and its entire subtree, mirroring
    /// `PermanentlyDeleteFolder`.
    pub fn delete_folder(&mut self, folder_id: FolderId) {
        // Collect all folder ids in the subtree (BFS).
        let mut stack = vec![folder_id];
        let mut all_folders = vec![];
        let mut all_schemes = vec![];
        while let Some(fid) = stack.pop() {
            all_folders.push(fid);
            if let Some(folder) = self.workspace.folders.get(&fid) {
                for child in &folder.children {
                    match child {
                        NodeRef::Folder(id) => stack.push(*id),
                        NodeRef::Scheme(id) => all_schemes.push(*id),
                    }
                }
            }
        }
        // Detach from parent.
        if let Some(folder) = self.workspace.folders.get(&folder_id) {
            if let Some(parent_id) = folder.parent {
                if let Some(parent) = self.workspace.folders.get_mut(&parent_id) {
                    parent
                        .children
                        .retain(|child| *child != NodeRef::Folder(folder_id));
                }
            }
        }
        // Remove archive state for folder and contained schemes.
        for fid in &all_folders {
            self.workspace
                .recently_deleted_folders
                .retain(|id| id != fid);
            self.workspace.deleted_folder_origins.remove(fid);
            self.workspace.folders.remove(fid);
        }
        for sid in &all_schemes {
            self.workspace.recently_deleted.retain(|id| id != sid);
            self.workspace.deleted_scheme_origins.remove(sid);
            self.workspace.schemes.remove(sid);
        }
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    /// Rename a folder.
    pub fn rename_folder(&mut self, folder_id: FolderId, name: &str) {
        if let Some(folder) = self.workspace.folders.get_mut(&folder_id) {
            folder.name = name.to_string();
        }
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    /// Move a scheme to the root folder (detach from wherever it currently lives).
    pub fn move_scheme_to_root(&mut self, scheme_id: SchemeId) {
        let root = self.workspace.root;
        for folder in self.workspace.folders.values_mut() {
            folder
                .children
                .retain(|child| *child != NodeRef::Scheme(scheme_id));
        }
        self.workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));
        self.record_changes(WorkspaceCrdtChangeSet::default().workspace());
    }

    /// Change the marker on a specific item in a scheme.
    pub fn set_item_marker(&mut self, scheme_id: SchemeId, item_index: usize, marker: ItemMarker) {
        let items = &mut self.scheme_mut(scheme_id).items;
        if item_index < items.len() {
            items[item_index].marker = marker;
            self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
        }
    }

    /// Set start/end dates on an item.  Automatically applies `Checkbox` marker.
    pub fn set_item_dates(
        &mut self,
        scheme_id: SchemeId,
        item_index: usize,
        start: Option<chrono::DateTime<chrono::Utc>>,
        end: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let items = &mut self.scheme_mut(scheme_id).items;
        if item_index < items.len() {
            let item = &mut items[item_index];
            item.marker = ItemMarker::Checkbox;
            item.start = start;
            item.end = end;
            self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
        }
    }

    /// Change the indent level on an item.
    pub fn set_item_indent(&mut self, scheme_id: SchemeId, item_index: usize, indent: u8) {
        let items = &mut self.scheme_mut(scheme_id).items;
        if item_index < items.len() {
            items[item_index].indent = indent;
            self.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id));
        }
    }

    /// Push a notification schedule change and return the server's
    /// `notification_schedule_revision`.
    ///
    /// Runs a full sync cycle first (which pushes any pending doc edits via the normal
    /// engine path), then sends a dedicated push with `notification_schedule_changed =
    /// true` and the supplied `sequence`/`hash`.  Includes the workspace document as a
    /// required payload (the real backend rejects pushes with zero documents).
    /// `hash` must be a 64-char hex string to pass real-backend validation.
    pub fn update_notification_schedule_with(
        &mut self,
        transport: &dyn SyncTransport,
        sequence: u64,
        hash: &str,
    ) -> u64 {
        // First run a normal sync to flush any pending doc edits.
        self.try_sync_with(transport)
            .expect("sync before schedule update");

        let now = Utc::now();
        let schedule = NotificationScheduleSnapshot {
            sequence,
            hash: hash.to_string(),
            window_start: now,
            window_end: now + chrono::Duration::hours(1),
            occurrence_count: 0,
        };

        // The real backend requires at least one document in the push body.
        // Include a fresh workspace snapshot so the push is always well-formed.
        // Using `full_snapshot_updates` produces an idempotent update (re-applying
        // it on the server is safe; it only bumps seq).
        let workspace_doc_update = self
            .store_crdt
            .full_snapshot_updates()
            .updates
            .into_iter()
            .find(|u| u.document == self.workspace.sync.id)
            .expect("workspace document must be in full snapshot");
        let request = BatchPushRequest {
            replica_id: self.replica_id,
            documents: vec![PushDocumentUpdates {
                document: workspace_doc_update.document,
                kind: workspace_doc_update.kind,
                updates: vec![workspace_doc_update.update_v1],
            }],
            notification_schedule_changed: true,
            notification_schedule: Some(schedule),
        };
        let response = transport
            .push(&request)
            .expect("notification schedule push");
        response.notification_schedule_revision
    }

    /// Upload media via the HTTP client.
    pub fn upload_media_to_http(
        &mut self,
        client: &http_transport::HttpClient,
        remote_latest: &HashMap<DocumentId, u64>,
    ) -> anyhow::Result<()> {
        use sha2::{Digest, Sha256};
        let refs: Vec<(DocumentId, String)> = self
            .workspace
            .schemes
            .iter()
            .filter_map(|(scheme_id, scheme)| {
                let meta = self.workspace.scheme_sync.get(scheme_id)?;
                Some((meta.id, scheme))
            })
            .flat_map(|(document, scheme)| {
                scheme.items.iter().flat_map(move |item| {
                    item_image_assets(item).into_iter().map(move |media| {
                        let image_name = format!("{}.{}", media.asset, media.format.extension());
                        (document, image_name)
                    })
                })
            })
            .collect();
        for (document, image_name) in refs {
            let Some(bytes) = self.media_assets.get(&image_name).cloned() else {
                continue;
            };
            if bytes.is_empty() {
                continue;
            }
            let byte_length = bytes.len() as u64;
            let digest = Sha256::digest(&bytes);
            let sha256: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            if !self.local_state.should_upload_media_asset(
                &image_name,
                document,
                byte_length,
                &sha256,
                remote_latest,
            ) {
                continue;
            }
            client.upload_media(document, &image_name, &bytes)?;
            self.local_state
                .mark_media_uploaded(image_name, document, byte_length, sha256);
        }
        Ok(())
    }

    /// Download media via the HTTP client.
    pub fn download_media_from_http(
        &mut self,
        client: &http_transport::HttpClient,
    ) -> anyhow::Result<()> {
        let refs: Vec<(DocumentId, String)> = self
            .workspace
            .schemes
            .keys()
            .filter_map(|scheme_id| {
                let meta = self.workspace.scheme_sync.get(scheme_id)?;
                let scheme = self.workspace.schemes.get(scheme_id)?;
                Some((meta.id, scheme.items.clone()))
            })
            .flat_map(|(document, items)| {
                items.into_iter().flat_map(move |item| {
                    item_image_assets(&item).into_iter().map({
                        move |media| {
                            let image_name =
                                format!("{}.{}", media.asset, media.format.extension());
                            (document, image_name)
                        }
                    })
                })
            })
            .collect();
        for (document, image_name) in refs {
            if self.media_assets.contains_key(&image_name) {
                continue;
            }
            if let Some(bytes) = client.download_media(document, &image_name)? {
                self.media_assets.insert(image_name, bytes);
            }
        }
        Ok(())
    }

    fn scheme_mut(&mut self, scheme_id: SchemeId) -> &mut Scheme {
        self.workspace
            .schemes
            .get_mut(&scheme_id)
            .unwrap_or_else(|| panic!("unknown scheme {scheme_id}"))
    }

    /// Public alias for tests that need to directly mutate a scheme (e.g. simulating
    /// a gsync re-import that removes or changes items without going through helpers).
    pub fn scheme_mut_pub(&mut self, scheme_id: SchemeId) -> &mut Scheme {
        self.scheme_mut(scheme_id)
    }

    // --- sync loop (the real engine) ---

    /// Run one full sync cycle.  Panics if the pull or push fails.
    /// Use `try_sync` instead when you need to observe push failures
    /// (e.g. `crdt_schema_invalid` regression tests).
    fn sync(&mut self, server: &TestServer) {
        self.try_sync(server).expect("sync");
    }

    /// Run one full sync cycle, returning `Err` if the push phase fails (e.g. the
    /// server returns `crdt_schema_invalid`).  The pull phase always panics on
    /// failure (pull errors indicate harness bugs, not the bugs under test).
    /// On push failure the pull has already been applied and `crdt_states` updated,
    /// mirroring the desktop's partial-progress-on-failure contract.
    pub fn try_sync(&mut self, server: &TestServer) -> anyhow::Result<()> {
        self.last_skipped.clear();
        self.workspace
            .canonicalize_personal_sync_identity(self.account_workspace);
        self.workspace.ensure_sync_metadata();

        // The pull/apply path reconstructs the CRDT from persisted state with the
        // device's deterministic clientID (`from_states`) — desktop
        // `sync_service::crdt_docs` and mobile's `self.crdt`, now restored rather than
        // rebuilt-from-plain-data. Because the clientID is stable, this instance and
        // the store CRDT share one Yjs identity, so remote merged state integrates
        // without competing re-encodings.
        let mut apply_crdt = WorkspaceCrdtDocuments::from_states(
            &self.workspace,
            self.replica_id,
            &self.crdt_states,
        )
        .expect("restore apply crdt");
        let workspace = self.workspace.clone();
        let pull = batch_pull_and_apply(
            server,
            &mut apply_crdt,
            &mut self.local_state,
            workspace,
            self.replica_id,
        )
        .expect("pull/apply");
        self.last_skipped = pull.skipped.clone();
        self.workspace = pull.workspace;
        let mut repaired_workspace_changed = self
            .workspace
            .canonicalize_personal_sync_identity(self.account_workspace);
        repaired_workspace_changed |= self.workspace.normalize_one_level_folders();
        if repaired_workspace_changed {
            // Repair deltas come from the same restored apply CRDT, mirroring
            // `sync_service::queue_repair_crdt_updates` and mobile's repair path.
            let outcome = apply_crdt.sync_changes(
                &self.workspace,
                &WorkspaceCrdtChangeSet::default().workspace(),
            );
            assert!(outcome.is_ok(), "{:?}", outcome.errors);
            let operation_id = OperationId::new();
            let local_sequence = self.next_sequence;
            self.next_sequence += 1;
            for update in outcome.updates {
                self.local_state.push_pending(PendingCrdtEdit {
                    operation_id,
                    workspace_id: self.workspace.id,
                    replica_id: self.replica_id,
                    local_sequence,
                    created_at: Utc::now(),
                    document: update.document,
                    kind: update.kind,
                    update_v1: update.update_v1,
                });
            }
        }

        let remote_latest: HashMap<DocumentId, u64> = self
            .local_state
            .document_cursors
            .values()
            .map(|cursor| (cursor.document, cursor.last_pulled_sequence))
            .collect();
        queue_workspace_bootstrap_updates(
            &mut self.local_state,
            &mut apply_crdt,
            &self.workspace,
            self.replica_id,
            &remote_latest,
        );

        // Persist the merged CRDT state (remote applied + repair + bootstrap heal).
        // This is what the desktop background thread writes to disk and what the UI
        // store reloads.
        self.crdt_states = apply_crdt.document_states();

        let schedule = test_notification_schedule();
        let mut pushed = Vec::new();
        // Propagate push errors to the caller instead of panicking — this is the
        // key harness change: the real transport returns Err on crdt_schema_invalid
        // and so should the test harness.
        let push_result = batch_push_pending(
            server,
            &mut self.local_state,
            self.replica_id,
            &schedule,
            &mut pushed,
            &mut apply_crdt,
            &self.workspace,
        );
        // Mirror desktop: the push's self-heal may repopulate a schema-less
        // document; persist the healed identity.
        self.crdt_states = apply_crdt.document_states();
        push_result?;

        // Mirror desktop `replace_workspace_from_sync` / mobile reload: the store
        // CRDT is reloaded from the persisted (merged) state — keeping its stable
        // deterministic identity — so later local diffs chain from the merged state
        // the server already has, instead of from a freshly minted clientID.
        self.store_crdt = WorkspaceCrdtDocuments::from_states(
            &self.workspace,
            self.replica_id,
            &self.crdt_states,
        )
        .expect("reload store crdt");
        Ok(())
    }

    /// Run one full sync cycle against any [`SyncTransport`] implementation.
    /// Equivalent to `try_sync` but transport-agnostic — integration tests pass
    /// an `HttpTransport` pointing at the real backend; the in-memory tests keep
    /// using `try_sync`.
    pub fn try_sync_with(&mut self, transport: &dyn SyncTransport) -> anyhow::Result<()> {
        self.last_skipped.clear();
        self.workspace
            .canonicalize_personal_sync_identity(self.account_workspace);
        self.workspace.ensure_sync_metadata();

        let mut apply_crdt = WorkspaceCrdtDocuments::from_states(
            &self.workspace,
            self.replica_id,
            &self.crdt_states,
        )
        .expect("restore apply crdt");
        let workspace = self.workspace.clone();
        let pull = batch_pull_and_apply(
            transport,
            &mut apply_crdt,
            &mut self.local_state,
            workspace,
            self.replica_id,
        )
        .expect("pull/apply");
        self.last_skipped = pull.skipped.clone();
        self.workspace = pull.workspace;
        let mut repaired_workspace_changed = self
            .workspace
            .canonicalize_personal_sync_identity(self.account_workspace);
        repaired_workspace_changed |= self.workspace.normalize_one_level_folders();
        if repaired_workspace_changed {
            let outcome = apply_crdt.sync_changes(
                &self.workspace,
                &WorkspaceCrdtChangeSet::default().workspace(),
            );
            assert!(outcome.is_ok(), "{:?}", outcome.errors);
            let operation_id = OperationId::new();
            let local_sequence = self.next_sequence;
            self.next_sequence += 1;
            for update in outcome.updates {
                self.local_state.push_pending(PendingCrdtEdit {
                    operation_id,
                    workspace_id: self.workspace.id,
                    replica_id: self.replica_id,
                    local_sequence,
                    created_at: Utc::now(),
                    document: update.document,
                    kind: update.kind,
                    update_v1: update.update_v1,
                });
            }
        }
        let remote_latest: HashMap<DocumentId, u64> = self
            .local_state
            .document_cursors
            .values()
            .map(|cursor| (cursor.document, cursor.last_pulled_sequence))
            .collect();
        queue_workspace_bootstrap_updates(
            &mut self.local_state,
            &mut apply_crdt,
            &self.workspace,
            self.replica_id,
            &remote_latest,
        );
        self.crdt_states = apply_crdt.document_states();
        let schedule = test_notification_schedule();
        let mut pushed = Vec::new();
        let push_result = batch_push_pending(
            transport,
            &mut self.local_state,
            self.replica_id,
            &schedule,
            &mut pushed,
            &mut apply_crdt,
            &self.workspace,
        );
        self.crdt_states = apply_crdt.document_states();
        push_result?;
        self.store_crdt = WorkspaceCrdtDocuments::from_states(
            &self.workspace,
            self.replica_id,
            &self.crdt_states,
        )
        .expect("reload store crdt");
        Ok(())
    }

    /// Remote latest after the most recent sync (cursors map). Useful for
    /// integration tests that need to pass `remote_latest` to media helpers.
    pub fn remote_latest_after_sync(&self) -> HashMap<DocumentId, u64> {
        self.local_state
            .document_cursors
            .values()
            .map(|cursor| (cursor.document, cursor.last_pulled_sequence))
            .collect()
    }

    pub fn record_changes(&mut self, changes: WorkspaceCrdtChangeSet) {
        self.workspace
            .canonicalize_personal_sync_identity(self.account_workspace);
        self.workspace.ensure_sync_metadata();
        let outcome = self.store_crdt.sync_changes(&self.workspace, &changes);
        assert!(outcome.is_ok(), "{:?}", outcome.errors);
        // Persist the store CRDT after every edit, as the desktop store does, so the
        // next sync's restored apply CRDT sees the local edits' base state.
        self.crdt_states = self.store_crdt.document_states();
        if outcome.updates.is_empty() {
            return;
        }
        let operation_id = OperationId::new();
        let local_sequence = self.next_sequence;
        self.next_sequence += 1;
        for update in outcome.updates {
            self.local_state.push_pending(PendingCrdtEdit {
                operation_id,
                workspace_id: self.workspace.id,
                replica_id: self.replica_id,
                local_sequence,
                created_at: Utc::now(),
                document: update.document,
                kind: update.kind,
                update_v1: update.update_v1,
            });
        }
    }

    // --- inspection ---

    /// Read-only view of the persisted sync state (stand-in for `sync-state.json`).
    pub fn local_state_ref(&self) -> &LocalSyncState {
        &self.local_state
    }

    pub fn root_scheme_ids(&self) -> Vec<SchemeId> {
        self.workspace
            .folder(self.workspace.root)
            .map(|folder| {
                folder
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        NodeRef::Scheme(id) => Some(*id),
                        NodeRef::Folder(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn scheme_item_texts(&self, scheme: SchemeId) -> Vec<String> {
        self.workspace.schemes[&scheme]
            .items
            .iter()
            .map(|item| item.text())
            .collect()
    }

    fn summary(&self) -> WorkspaceSummary {
        let mut schemes = self
            .workspace
            .schemes
            .iter()
            .map(|(id, scheme)| SchemeSummary {
                id: id.to_string(),
                name: scheme.name.clone(),
                archived: self.workspace.is_scheme_deleted(*id),
                gsync: scheme.gsync,
                source: scheme_source_label(&scheme.source),
                items: scheme.items.iter().map(item_summary).collect(),
            })
            .collect::<Vec<_>>();
        schemes.sort_by(|left, right| left.id.cmp(&right.id));

        let mut folders = self
            .workspace
            .folders
            .values()
            .map(|folder| FolderSummary {
                id: folder.id.to_string(),
                name: folder.name.clone(),
                children: folder.children.iter().map(node_ref_label).collect(),
            })
            .collect::<Vec<_>>();
        folders.sort_by(|left, right| left.id.cmp(&right.id));

        WorkspaceSummary {
            workspace_document: self.workspace.sync.id.to_string(),
            root_schemes: self
                .root_scheme_ids()
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            folders,
            recently_deleted: self
                .workspace
                .recently_deleted
                .iter()
                .map(ToString::to_string)
                .collect(),
            daily_queue: self
                .workspace
                .daily_queue
                .iter()
                .map(|(date, scheme)| (date.to_string(), scheme.to_string()))
                .collect(),
            schemes,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct WorkspaceSummary {
    workspace_document: String,
    root_schemes: Vec<String>,
    folders: Vec<FolderSummary>,
    recently_deleted: Vec<String>,
    daily_queue: Vec<(String, String)>,
    schemes: Vec<SchemeSummary>,
}

#[derive(Debug, Eq, PartialEq)]
struct FolderSummary {
    id: String,
    name: String,
    children: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct SchemeSummary {
    id: String,
    name: String,
    archived: bool,
    gsync: bool,
    source: String,
    items: Vec<String>,
}

fn node_ref_label(node: &NodeRef) -> String {
    match node {
        NodeRef::Folder(id) => format!("folder:{id}"),
        NodeRef::Scheme(id) => format!("scheme:{id}"),
    }
}

fn item_summary(item: &Item) -> String {
    serde_json::to_string(item).expect("item should serialize")
}

/// A stable label for a scheme's source so the convergence check catches a lost or
/// diverged imported-calendar association (provider/account/calendar), not just the
/// local-vs-imported distinction.
fn scheme_source_label(source: &SchemeSource) -> String {
    match source {
        SchemeSource::Local => "local".to_string(),
        SchemeSource::ImportedCalendar(source) => format!(
            "imported:{:?}:{}:{}:{}:{}",
            source.provider,
            source.account_id,
            source.account_email.as_deref().unwrap_or(""),
            source.calendar_id,
            source.read_only,
        ),
    }
}

// ---------------------------------------------------------------------------
// Daily-queue carryover predicates
//
// Faithful copies of the private helpers in `knotq_state::daily_queue` (the source
// of truth for the "roll over from yesterday" action). They are duplicated here
// rather than imported because `knotq-state` depends on `knotq-sync`, so the sync
// crate cannot dev-depend on it without a cycle. Keep them in sync with
// `desktop/state/src/daily_queue.rs`.
// ---------------------------------------------------------------------------

/// How many days back the carryover scans for the most recent day with content.
const DQ_CARRYOVER_LOOKBACK_DAYS: i64 = 14;

fn dq_last_nonblank_day(workspace: &Workspace, today: NaiveDate) -> Option<NaiveDate> {
    (1..=DQ_CARRYOVER_LOOKBACK_DAYS)
        .map(|offset| today - Duration::days(offset))
        .find(|date| {
            workspace
                .daily_queue_scheme_id(*date)
                .and_then(|id| workspace.scheme(id))
                .is_some_and(|scheme| !dq_scheme_is_blank(scheme))
        })
}

fn dq_scheme_is_blank(scheme: &Scheme) -> bool {
    if scheme.items.is_empty() {
        return true;
    }
    scheme
        .items
        .first()
        .is_some_and(dq_item_is_blank_placeholder)
        && scheme.items.len() == 1
}

fn dq_item_is_blank_placeholder(item: &Item) -> bool {
    item.text().trim().is_empty()
        && !item.has_images()
        && !item.has_table()
        && item.marker == ItemMarker::Blank
        && item.indent == 0
        && !dq_item_has_annotations(item)
        && item.priority.is_none()
        && item.state.len() == 1
        && item.state[0].state.progress == 0
        && item.state[0].state.notification_offset_secs.is_none()
}

fn dq_item_is_fully_complete_task(item: &Item) -> bool {
    item.marker == ItemMarker::Checkbox
        && !item.state.is_empty()
        && item.state.iter().all(|state| state.state.is_done())
}

fn dq_item_has_annotations(item: &Item) -> bool {
    item.start.is_some() || item.end.is_some() || item.available.is_some() || item.repeats.is_some()
}

fn dq_strip_annotations(item: &mut Item) {
    item.start = None;
    item.end = None;
    item.available = None;
    item.repeats = None;
}

fn test_notification_schedule() -> NotificationScheduleSnapshot {
    let now = Utc::now();
    NotificationScheduleSnapshot {
        sequence: 0,
        // The real backend requires a 64-char sha256 hex hash and a non-empty
        // window (window_end > window_start).
        hash: "0".repeat(64),
        window_start: now,
        window_end: now + chrono::Duration::hours(1),
        occurrence_count: 0,
    }
}

/// Merge a stored merged state plus a batch of v1 updates into a new merged state,
/// exactly as the worker's `validateAndCompactCrdtUpdates` does.
fn merge_state(base: &[u8], updates: &[Vec<u8>]) -> Vec<u8> {
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

/// Tiny deterministic PRNG (SplitMix64) so fuzz runs are reproducible.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E3779B97F4A7C15))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next() % bound
        }
    }
}
