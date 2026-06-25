//! `TestDevice` sync loop (the real engine), change recording, and inspection.
use super::*;

impl TestDevice {
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
        // Server-authoritative per-document latest (from the pull's known_documents),
        // exactly as desktop snapshot.rs / mobile sync_once use it — not a
        // cursor-derived approximation. This makes the bootstrap's re-seed decisions
        // mirror production so the fuzzer's results are trustworthy.
        let remote_latest = pull.remote_latest.clone();
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

        // remote_latest was captured from the server-authoritative pull above.
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
        // Server-authoritative per-document latest (from the pull's known_documents),
        // exactly as desktop snapshot.rs / mobile sync_once use it — not a
        // cursor-derived approximation. This makes the bootstrap's re-seed decisions
        // mirror production so the fuzzer's results are trustworthy.
        let remote_latest = pull.remote_latest.clone();
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
        // remote_latest was captured from the server-authoritative pull above.
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

    pub(super) fn summary(&self) -> WorkspaceSummary {
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
