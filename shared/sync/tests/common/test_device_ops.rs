//! `TestDevice` workspace operations, the real sync loop, and inspection helpers.
use super::*;

impl TestDevice {
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

    pub(super) fn scheme_mut(&mut self, scheme_id: SchemeId) -> &mut Scheme {
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
}
