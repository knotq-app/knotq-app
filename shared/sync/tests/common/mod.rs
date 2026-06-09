//! Shared in-memory, engine-driven multi-device test harness.
//!
//! There is **no network**: [`TestServer`] implements the real [`SyncTransport`]
//! trait against an in-process `HashMap`, mirroring the production worker's
//! merged-state model — one merged Yjs `state_v1` per document, bumped by a `seq`
//! on each push. Devices sync through the *actual* shared engine
//! ([`batch_pull_and_apply`] + [`batch_push_pending`]) and the real CRDT layer, so
//! these tests exercise exactly the code desktop and mobile run, end to end.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use chrono::Utc;
use knotq_model::{
    daily_queue_scheme_id, CalendarProvider, DocumentId, Folder, FolderId, ImportedCalendarSource,
    Item, NodeRef, OperationId, ReplicaId, Scheme, SchemeId, SchemeSource, SyncDocumentKind,
    Workspace, WorkspaceId,
};
use knotq_sync::{
    batch_pull_and_apply, batch_push_pending, queue_workspace_bootstrap_updates,
    validate_crdt_update_sequence, BatchPullRequest, BatchPullResponse, BatchPushRequest,
    BatchPushResponse, LocalSyncState, NotificationScheduleSnapshot, PendingCrdtEdit,
    PulledCrdtDocument, PushDocumentUpdates, PushedCrdtDocument, SyncTransport,
    WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments,
};
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct DeviceKey(pub usize);

pub const D0: DeviceKey = DeviceKey(0);
pub const D1: DeviceKey = DeviceKey(1);
pub const D2: DeviceKey = DeviceKey(2);

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

pub struct Harness {
    account_workspace: WorkspaceId,
    base: Workspace,
    server: TestServer,
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
            server: TestServer::default(),
            devices: BTreeMap::new(),
            device_count,
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

    pub fn set_daily_queue(
        &mut self,
        key: DeviceKey,
        date: chrono::NaiveDate,
        lines: &[&str],
    ) -> SchemeId {
        self.device_mut(key).set_daily_queue(date, lines)
    }

    pub fn sync(&mut self, key: DeviceKey) {
        let mut device = self.devices.remove(&key).expect("missing device");
        device.sync(&self.server);
        self.devices.insert(key, device);
    }

    pub fn server_pull_calls(&self) -> usize {
        self.server.pull_calls()
    }

    pub fn server_push_calls(&self) -> usize {
        self.server.push_calls()
    }

    pub fn server_document_count(&self) -> usize {
        self.server.document_count()
    }

    pub fn push_remote_workspace_snapshot(&self, workspace: &Workspace) {
        let documents = WorkspaceCrdtDocuments::snapshot_updates(workspace)
            .updates
            .into_iter()
            .map(|update| PushDocumentUpdates {
                document: update.document,
                kind: update.kind,
                updates: vec![update.update_v1],
            })
            .collect::<Vec<_>>();
        self.server
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

#[derive(Default)]
pub struct TestServer {
    documents: RefCell<HashMap<DocumentId, ServerDocument>>,
    counters: RefCell<ServerCounters>,
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

    fn push(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        self.counters.borrow_mut().push_calls += 1;
        let mut documents = self.documents.borrow_mut();
        let mut out = Vec::new();
        for doc in &request.documents {
            let entry = documents
                .entry(doc.document)
                .or_insert_with(|| ServerDocument {
                    kind: doc.kind,
                    seq: 0,
                    state_v1: Vec::new(),
                });
            assert_eq!(
                entry.kind, doc.kind,
                "document kind changed under {}",
                doc.document
            );
            // Validate the merged base + incoming updates the way the worker does,
            // then fold them into a single merged state — there is no delta log.
            let mut chain: Vec<&[u8]> = Vec::new();
            if !entry.state_v1.is_empty() {
                chain.push(entry.state_v1.as_slice());
            }
            chain.extend(doc.updates.iter().map(|u| u.as_slice()));
            if let Err(err) = validate_crdt_update_sequence(doc.kind, chain.iter().copied()) {
                panic!(
                    "server rejected an invalid update chain for {:?} {} (had_base={}, updates={}): {err:#}",
                    doc.kind,
                    doc.document,
                    !entry.state_v1.is_empty(),
                    doc.updates.len(),
                );
            }
            entry.state_v1 = merge_state(&entry.state_v1, &doc.updates);
            entry.seq += 1;
            out.push(PushedCrdtDocument {
                document: doc.document,
                seq: entry.seq,
                accepted: doc.updates.len(),
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
}

impl TestDevice {
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
        }
    }

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
            items[index].text = text.to_string();
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
        if !self.workspace.folders.contains_key(&folder_id) {
            self.workspace.folders.insert(
                folder_id,
                Folder {
                    id: folder_id,
                    name: "Restored".to_string(),
                    parent: Some(root),
                    children: Vec::new(),
                    expanded: true,
                },
            );
        }
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

    fn scheme_mut(&mut self, scheme_id: SchemeId) -> &mut Scheme {
        self.workspace
            .schemes
            .get_mut(&scheme_id)
            .unwrap_or_else(|| panic!("unknown scheme {scheme_id}"))
    }

    // --- sync loop (the real engine) ---

    fn sync(&mut self, server: &TestServer) {
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

        // Persist the merged CRDT state (remote applied + any repair). This is what
        // the desktop background thread writes to disk and what the UI store reloads.
        self.crdt_states = apply_crdt.document_states();

        let remote_latest: HashMap<DocumentId, u64> = self
            .local_state
            .document_cursors
            .values()
            .map(|cursor| (cursor.document, cursor.last_pulled_sequence))
            .collect();
        queue_workspace_bootstrap_updates(
            &mut self.local_state,
            &apply_crdt,
            &self.workspace,
            self.replica_id,
            &remote_latest,
        );

        let schedule = test_notification_schedule();
        let mut pushed = Vec::new();
        batch_push_pending(
            server,
            &mut self.local_state,
            self.replica_id,
            &schedule,
            &mut pushed,
        )
        .expect("push");

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
    }

    fn record_changes(&mut self, changes: WorkspaceCrdtChangeSet) {
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
            .map(|item| item.text.clone())
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
                items: scheme.items.iter().map(|item| item.text.clone()).collect(),
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

fn test_notification_schedule() -> NotificationScheduleSnapshot {
    let now = Utc::now();
    NotificationScheduleSnapshot {
        sequence: 0,
        hash: "test".to_string(),
        window_start: now,
        window_end: now,
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
