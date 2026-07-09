//! Inherent operations and assertions on [`Harness`].
use super::*;

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

    /// Push-only cycle for `key` (no preceding pull) — see
    /// `TestDevice::try_push_only`. In-memory harness only.
    pub fn device_push_only(&mut self, key: DeviceKey) -> anyhow::Result<()> {
        let device = self.devices.get_mut(&key).expect("device");
        match &self.backend {
            HarnessBackend::InMemory(server) => device.try_push_only(server),
            HarnessBackend::Http(_) => panic!("push-only is in-memory only"),
        }
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
    /// Server-side effect of an accepted `POST /v1/sync/squash` built by `key`:
    /// replace the scheme's stored content document with that device's
    /// history-free rebuild, bumping seq AND epoch. Returns (seq, epoch).
    pub fn squash_scheme_on_server(&self, key: DeviceKey, scheme: SchemeId) -> (u64, u64) {
        let device = self.device(key);
        let document = device.scheme_document_id(scheme);
        let rebuilt = device.rebuild_scheme_state(scheme);
        self.require_in_memory_server()
            .squash_document(document, rebuilt)
    }

    /// The epoch `key` last recorded for a scheme's content document.
    pub fn scheme_epoch(&self, key: DeviceKey, scheme: SchemeId) -> u64 {
        self.device(key).scheme_document_epoch(scheme)
    }

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
                epoch: 0,
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
