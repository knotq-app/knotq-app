//! `TestDevice` construction, restart simulation, account switch, and media helpers.
use super::*;

impl TestDevice {
    /// Construct a `TestDevice` from a `Workspace` and canonical `account_workspace` id.
    /// Used by integration tests that need to share the backend's provisioned workspace_id.
    pub fn new_from_base(base: &Workspace, account_workspace: WorkspaceId) -> Self {
        Self::from_base(base, account_workspace)
    }

    pub(super) fn from_base(base: &Workspace, account_workspace: WorkspaceId) -> Self {
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

    /// Read-only view of the local sync state (cursors, pending) for assertions.
    pub fn local_state(&self) -> &LocalSyncState {
        &self.local_state
    }

    /// Number of per-document pull/push cursors currently tracked.
    pub fn document_cursor_count(&self) -> usize {
        self.local_state.document_cursors.len()
    }

    /// Number of media upload cursors currently tracked.
    pub fn media_cursor_count(&self) -> usize {
        self.local_state.media_cursors.len()
    }

    /// `true` when the materialized workspace holds a scheme named `name`.
    pub fn has_scheme_named(&self, name: &str) -> bool {
        self.workspace.schemes.values().any(|s| s.name == name)
    }

    /// Number of items (lines) in the first scheme named `name`, or `None` if absent.
    /// `Some(0)` distinguishes "present but content doc missing" from `None`.
    pub fn scheme_line_count(&self, name: &str) -> Option<usize> {
        self.workspace
            .schemes
            .values()
            .find(|s| s.name == name)
            .map(|s| s.items.len())
    }

    /// `true` when the materialized workspace holds a folder named `name`.
    pub fn has_folder_named(&self, name: &str) -> bool {
        self.workspace.folders.values().any(|f| f.name == name)
    }

    /// `true` when `scheme_id` is in the archive (recently_deleted) set.
    pub fn scheme_is_archived(&self, scheme_id: SchemeId) -> bool {
        self.workspace.recently_deleted.contains(&scheme_id)
    }

    /// `true` when this device's materialized workspace is content-identical to
    /// `other` — the same convergence definition `assert_all_converged` uses
    /// (`summary()`), exposed so multi-account model tests in other files can assert
    /// that every device on an account converged.
    pub fn converges_with(&self, other: &TestDevice) -> bool {
        self.summary() == other.summary()
    }

    // --- account switch (sign out of A, sign into B) ---------------------------

    /// Adopt a different account's canonical workspace identity and re-label the live
    /// workspace CRDT document to the new id, preserving its content (mobile
    /// `reidentify_workspace_document` / the desktop `snapshot.rs` re-key). Re-encoding
    /// the doc under the new id — rather than only moving persisted bytes between map
    /// keys — keeps it consistent across *repeated* switches (A -> B -> A). Returns the
    /// re-identified full-state update (None when the workspace id is unchanged) so the
    /// caller can queue it for push, exactly as the real drivers do. Does NOT touch
    /// cursors.
    fn reidentify_for_account(
        &mut self,
        new_account_workspace: WorkspaceId,
    ) -> Option<CrdtDocumentUpdate> {
        self.account_workspace = new_account_workspace;
        self.workspace
            .canonicalize_personal_sync_identity(new_account_workspace);
        self.workspace.ensure_sync_metadata();
        let update = self
            .store_crdt
            .reidentify_workspace_document(self.workspace.sync.id)
            .expect("re-identify workspace document");
        self.crdt_states = self.store_crdt.document_states();
        update
    }

    /// Queue the re-identified workspace document's full state for push, mirroring
    /// desktop `queue_reidentified_workspace_update` / mobile `sync_once`. Queued AFTER
    /// the cursor reset so it survives (the reset drops stale workspace-index pending).
    fn queue_reidentified_workspace(&mut self, update: CrdtDocumentUpdate) {
        let local_sequence = self.next_sequence;
        self.next_sequence += 1;
        self.local_state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: self.workspace.id,
            replica_id: self.replica_id,
            local_sequence,
            created_at: Utc::now(),
            document: update.document,
            kind: update.kind,
            update_v1: update.update_v1,
            touched_items: update.touched_items,
        });
    }

    /// Simulate signing out and into a DIFFERENT account (new workspace id + server),
    /// mirroring the fixed drivers: adopt the new canonical workspace identity, re-key
    /// the workspace CRDT document, reset the previous account's stale cursors via
    /// [`LocalSyncState::reset_for_account_change`], then queue the re-identified
    /// workspace state for push. This is desktop `configure_local_state` + the
    /// `snapshot.rs` re-key/queue / mobile `sync_once` + `reidentify_workspace_document`,
    /// distilled to the harness.
    pub fn switch_account(&mut self, new_account_workspace: WorkspaceId, new_server_url: &str) {
        let reidentified = self.reidentify_for_account(new_account_workspace);
        self.local_state
            .reset_for_account_change(self.workspace.id, new_server_url);
        self.local_state.workspace_id = Some(self.workspace.id);
        self.local_state.server_url = Some(new_server_url.to_string());
        if let Some(update) = reidentified {
            self.queue_reidentified_workspace(update);
        }
        // Force re-seed every scheme's full content to the new account. The bootstrap
        // only re-seeds documents the new server LACKS (remote_latest == 0); a scheme
        // the new account already holds from another origin (or empty) would otherwise
        // never receive this device's content — the cross-account content gap. Full
        // snapshots union idempotently, and with deterministic item creation items
        // dedupe instead of duplicating.
        knotq_sync::queue_account_switch_reseed(
            &mut self.local_state,
            &self.store_crdt,
            &self.workspace,
            self.replica_id,
        );
        self.next_sequence = self
            .local_state
            .pending
            .iter()
            .map(|edit| edit.local_sequence)
            .max()
            .unwrap_or(0)
            + 1;
    }

    /// Account switch WITHOUT the cursor reset — reproduces the pre-fix driver, which
    /// overwrote the account identity but reused the previous account's pull/push and
    /// media cursors. Use this to demonstrate the silent-skip / `crdt_schema_invalid`
    /// bug the reset fixes.
    pub fn switch_account_without_cursor_reset(
        &mut self,
        new_account_workspace: WorkspaceId,
        new_server_url: &str,
    ) {
        let reidentified = self.reidentify_for_account(new_account_workspace);
        self.local_state.workspace_id = Some(self.workspace.id);
        self.local_state.server_url = Some(new_server_url.to_string());
        if let Some(update) = reidentified {
            self.queue_reidentified_workspace(update);
        }
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
}
