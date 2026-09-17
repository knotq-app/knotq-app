use anyhow::Result;
use chrono::Utc;
use knotq_model::{
    OperationId, ReplicaId, SyncAccountSettings, SyncDocumentKind, Workspace, WorkspaceId,
};
use knotq_storage_json::{
    load_crdt_state, load_local_sync_state, load_workspace_with_options, save_crdt_state,
    save_local_sync_state, save_workspace, workspace_path, WorkspaceLoadOptions,
};
use knotq_sync::{
    batch_pull_and_apply, batch_push_pending, queue_account_switch_reseed,
    queue_workspace_bootstrap_updates, CrdtDocumentUpdate, LocalSyncState, PendingCrdtEdit,
    SkippedDocument, WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments,
};

use super::http::normalize_api_base;
use super::media::{download_missing_media_assets, upload_local_media_assets};
use super::{SyncEnvironment, SyncHttpClient, SyncRunResult, SyncSnapshot};

/// Run one sync against the configured backend and the app's data directory.
pub(super) fn sync_snapshot(snapshot: SyncSnapshot) -> Result<SyncRunResult> {
    let path = workspace_path();
    let image_dir = knotq_storage_json::image_assets_dir();
    let client = SyncHttpClient {
        api_base: normalize_api_base(&snapshot.account.api_base)?,
        bearer_token: snapshot.account.bearer_token.clone(),
    };
    // Batched pull/push prefer the live WebSocket and fall back to `client` (HTTP)
    // when the socket is down. Media transfer always uses `client` (HTTP) directly.
    let ws_sync = snapshot.ws_sync.clone();
    let transport = super::ws_transport::FallbackTransport::new(ws_sync.as_deref(), &client);
    sync_snapshot_in(
        SyncEnvironment {
            workspace_path: &path,
            image_dir: &image_dir,
            transport: &transport,
            side_channel: &client,
        },
        snapshot,
    )
}

/// The sync run itself, against an explicit data directory and backend.
pub(super) fn sync_snapshot_in(
    env: SyncEnvironment<'_>,
    snapshot: SyncSnapshot,
) -> Result<SyncRunResult> {
    let path = env.workspace_path;
    let mut workspace = workspace_for_background_sync(path, snapshot.workspace);
    // The notification schedule is computed here on the background sync thread, never
    // on main: recurrence expansion + per-occurrence JSON/SHA-256 hashing over the
    // whole workspace is the heaviest part of preparing a sync. When the caller
    // determined nothing schedule-relevant changed since the last run it hands back
    // that run's schedule in `reuse_schedule` and we skip the recompute outright.
    //
    // It is computed from `workspace` — the FULL on-disk workspace overlaid with the
    // in-memory edits — not the partial in-memory snapshot. That makes it independent
    // of which off-screen daily-queue schemes happen to be lazily loaded into memory
    // (so the reuse cache, keyed on the schedule generation, can't be invalidated by a
    // mere load), and it is strictly more complete: a device that never scrolled to a
    // future week still reports that week's notifications to the server.
    let notification_schedule = snapshot.reuse_schedule.clone().unwrap_or_else(|| {
        crate::notifications::notification_schedule_snapshot(
            &workspace,
            snapshot.notification_defaults,
            Utc::now(),
            0,
        )
    });
    let server_workspace_id = sync_workspace_id(&snapshot.account, workspace.id);
    // Capture the workspace document's id before adopting the account's canonical
    // identity, so an account switch can carry its content to the new id below.
    let previous_workspace_document_id = workspace.sync.id;
    let (_local_workspace_repair_needed, local_workspace_changed) =
        workspace.canonicalize_personal_sync_identity_with_change(server_workspace_id);
    workspace.ensure_sync_metadata();

    let mut local_state = load_local_sync_state(path).unwrap_or_default();
    // One-time recovery: clear stale pull cursors so this sync re-pulls and
    // re-merges every document, repairing any workspace left diverged by the earlier
    // push-failure desync.
    local_state.heal_for_recovery_version();
    configure_local_state(
        &mut local_state,
        server_workspace_id,
        snapshot.replica_id,
        &snapshot.account,
    );
    merge_pending(&mut local_state, snapshot.pending);
    for (operation, fields) in snapshot.queued_item_fields {
        local_state.record_queued_item_fields(operation, fields);
    }
    local_state.prune_queued_item_fields();

    let transport = env.transport;
    let client = env.side_channel;
    let image_dir = env.image_dir;
    // Restore the long-lived CRDT documents from disk and overlay the UI store's
    // latest states (the `snapshot`), so the sync's CRDT carries this device's stable
    // deterministic identity plus its newest local edits — never rebuilt from plain
    // data. Disk fills documents the in-memory store doesn't hold (e.g. archived /
    // off-screen Daily Queue schemes loaded by `workspace_for_background_sync`).
    // The store restored the documents it holds with the queue folded in; a
    // document only on disk (a day off screen) did not pass through it, so fold
    // the queue into that one here (`restored_crdt_states`).
    let store_documents: std::collections::HashSet<knotq_model::DocumentId> =
        snapshot.crdt_states.keys().copied().collect();
    let mut crdt_states: std::collections::HashMap<knotq_model::DocumentId, std::sync::Arc<[u8]>> =
        load_crdt_state(path)
            .unwrap_or_default()
            .into_iter()
            .map(|(document, state)| {
                let state = if store_documents.contains(&document) {
                    state
                } else {
                    knotq_sync::fold_pending_edits_into_state(
                        document,
                        &state,
                        &local_state.pending,
                    )
                    .unwrap_or(state)
                };
                (document, std::sync::Arc::from(state))
            })
            .collect();
    // Encode HERE, on the background sync thread: these handles were taken on the
    // UI thread precisely so this cost lands off main.
    crdt_states.extend(
        snapshot
            .crdt_states
            .into_iter()
            .map(|(document, handle)| (document, handle.encode())),
    );
    // When this device adopts a different account's canonical workspace id (a
    // sign-in into an account it did not last sync — e.g. prod -> sandbox), carry
    // the workspace document's persisted content to the new id. `from_states` keys
    // the workspace doc by `workspace.sync.id`; without this it rebuilds that doc
    // EMPTY (the new id is absent from `crdt_states`) and the workspace is then
    // materialized from an empty index, silently dropping every local scheme.
    // Re-keying preserves the content — identical to mobile's live re-label, since
    // `from_states` reconstructs the doc from these bytes under the new id — so the
    // pull merges (unions) the local and server workspace histories over the shared
    // id. The carried snapshot is queued for push so the server unions it in too;
    // `queue_workspace_bootstrap_updates` only force-pushes docs with no server base.
    let account_switched = workspace.sync.id != previous_workspace_document_id;
    let reidentified_workspace = if account_switched {
        crdt_states
            .remove(&previous_workspace_document_id)
            .map(|state| {
                crdt_states.insert(workspace.sync.id, state.clone());
                CrdtDocumentUpdate {
                    document: workspace.sync.id,
                    kind: SyncDocumentKind::PersonalWorkspace,
                    update_v1: state.to_vec(),
                    touched_items: Vec::new(),
                }
            })
    } else {
        None
    };
    let mut crdt_docs =
        WorkspaceCrdtDocuments::from_states(&workspace, snapshot.replica_id, &crdt_states)?;
    if let Some(update) = reidentified_workspace {
        queue_reidentified_workspace_update(
            &mut local_state,
            snapshot.replica_id,
            &workspace,
            update,
        );
    }
    let mut pushed = Vec::new();

    // One batched pull syncs the whole workspace: the server returns the current
    // merged state of every document whose seq advanced past our cursor (and any
    // document created on another device). Applying it materializes the merged
    // workspace; the engine applies the workspace index before scheme content so
    // newly discovered schemes route correctly.
    let pull = batch_pull_and_apply(
        transport,
        &mut crdt_docs,
        &mut local_state,
        workspace,
        snapshot.replica_id,
    )?;
    log_skipped_documents(&pull.skipped);
    // What this pull changed locally. A run that fails after saving its cursors
    // never lands, so these have to be pulled again (`forget_pull_of_failed_run`).
    let pulled_changes: Vec<knotq_model::DocumentId> =
        pull.changed_documents.iter().copied().collect();
    let mut workspace = pull.workspace;
    let remote_updates_applied = pull.remote_updates_applied;
    let locally_repaired_documents = pull.locally_repaired_documents;
    let (repaired_identity, repaired_identity_changed) =
        workspace.canonicalize_personal_sync_identity_with_change(server_workspace_id);
    let repaired_folders = workspace.normalize_one_level_folders();
    let repaired_markers = workspace.normalize_item_markers();
    let repaired_workspace_changed = repaired_identity || repaired_folders || repaired_markers;
    let repaired_workspace_persist_changed =
        repaired_identity_changed || repaired_folders || repaired_markers;
    if repaired_workspace_changed {
        queue_repair_crdt_updates(
            &mut local_state,
            &workspace,
            snapshot.replica_id,
            &mut crdt_docs,
        )?;
    }
    if account_switched {
        // Re-seed only after adopting the destination account's workspace index.
        // Queueing before the pull could retain a source-account-only scheme as an
        // orphan pending document; if the destination already had a tombstoned base
        // for that document, the server correctly rejected the orphan snapshot as
        // schema-invalid. The post-pull CRDT contains both local and destination
        // history, and the materialized workspace now defines the authoritative set
        // of scheme documents that may be pushed.
        let reseed_excluded = pull
            .skipped
            .iter()
            .map(|skipped| skipped.document)
            .collect();
        queue_account_switch_reseed(
            &mut local_state,
            &crdt_docs,
            &workspace,
            snapshot.replica_id,
            &reseed_excluded,
        );
    }

    upload_local_media_assets(
        client,
        image_dir,
        &mut local_state,
        &workspace,
        &pull.remote_latest,
    )?;
    let mut media_downloaded = download_missing_media_assets(client, image_dir, &workspace)?;

    let replica_id = local_state.replica_id.unwrap_or_default();
    // The server's per-document seq (our advanced pull cursor) tells the bootstrap
    // which documents the server already has a base for; the rest get a full snapshot
    // from the persistent CRDT (so the re-seed shares identity with this device's
    // diffs) queued before their deltas. The bootstrap also repairs schema-less
    // documents (a scheme created by a direct workspace mutation that never reached
    // the CRDT) by repopulating them from the workspace before snapshotting.
    let healed_documents = queue_workspace_bootstrap_updates(
        &mut local_state,
        &mut crdt_docs,
        &workspace,
        replica_id,
        &pull.remote_latest,
    );
    for document in &healed_documents {
        eprintln!("sync: repopulated schema-less CRDT document {document} before bootstrap");
    }

    // The CRDT documents are now final for this run (remote applied + repair +
    // bootstrap heal). Capture their merged state to hand back to the UI store and
    // to persist on disk.
    let merged_crdt_states = crdt_docs.document_states();

    // Persist the merged workspace BEFORE pushing. The durable pull cursors are
    // saved after the push regardless of its outcome, so the workspace must be on
    // disk first — otherwise a push failure would advance the cursor while
    // discarding the just-pulled remote schemes and archive (recently_deleted)
    // state, and the next sync (cursor already advanced) would never re-pull them.
    // That desync silently drops other devices' schemes and re-activates archived
    // ones. The CRDT state is saved in lockstep so a restart restores the same
    // documents (with their stable identity) — including any bootstrap-healed
    // documents, whose pushed snapshots must share identity with future local diffs.
    if remote_updates_applied > 0
        || local_workspace_changed
        || repaired_workspace_persist_changed
        || !locally_repaired_documents.is_empty()
        || !healed_documents.is_empty()
    {
        save_workspace(path, &workspace)?;
        save_crdt_state(path, &merged_crdt_states)?;
    }

    // Persist pull cursors, dropped orphans, and per-document push acks even if the
    // push below fails partway, so a transient push error never forces the next
    // sync to re-download every document from sequence zero. The merged workspace
    // above is already durable, so the cursor never runs ahead of it.
    // The records of the edits about to be pushed, before the push clears them.
    let queued_item_fields = local_state.queued_item_field_union();
    let push_result = batch_push_pending(
        transport,
        &mut local_state,
        replica_id,
        &notification_schedule,
        snapshot.reuse_schedule.is_none(),
        &mut pushed,
        &mut crdt_docs,
        &workspace,
    );
    // Until this run lands, a quit abandons these pulls (`abandon_unlanded_sync_run`).
    local_state.unlanded_pulls = pulled_changes.clone();
    if push_result.is_err() {
        // The run returns the push error and never lands; see
        // `forget_pull_of_failed_run`. Its acks above are still worth keeping.
        for document in &pulled_changes {
            local_state.reset_pull_cursor(*document);
        }
    }
    local_state.prune_queued_item_fields();
    save_local_sync_state(path, &local_state)?;
    // The push's own self-heal may have repopulated a schema-less document after
    // the capture above; persist the healed state so this device's future diffs
    // share its identity instead of re-minting the same clientID from clock zero.
    let merged_crdt_states = {
        let post_push_states = crdt_docs.document_states();
        if post_push_states != merged_crdt_states {
            save_crdt_state(path, &post_push_states)?;
        }
        post_push_states
    };
    push_result?;

    // Retry media after the CRDT push using a head map that treats newly pushed
    // documents as present, so successful pre-push uploads are not re-sent but
    // skipped or changed local assets still get uploaded.
    let mut media_remote_latest = pull.remote_latest;
    for pushed_document in &pushed {
        media_remote_latest
            .entry(pushed_document.document)
            .or_insert(1);
    }
    if let Err(err) = upload_local_media_assets(
        client,
        image_dir,
        &mut local_state,
        &workspace,
        &media_remote_latest,
    ) {
        forget_pull_of_failed_run(path, &mut local_state, &pulled_changes);
        return Err(err);
    }
    save_local_sync_state(path, &local_state)?;
    match download_missing_media_assets(client, image_dir, &workspace) {
        Ok(downloaded) => media_downloaded |= downloaded,
        Err(err) => {
            forget_pull_of_failed_run(path, &mut local_state, &pulled_changes);
            return Err(err);
        }
    }

    // Post-run maintenance: propose at most one history squash when the run left
    // this device fully synced. The proposal replaces a bloated scheme document's
    // server state with a history-free rebuild of identical content (epoch bump);
    // every server rejection — head moved, squashed too recently — is an
    // expected, benign skip. The squash rides HTTP even when the WebSocket is up
    // (it is rare and not latency-sensitive).
    let mut remote_updates_applied = remote_updates_applied;
    let mut merged_crdt_states = merged_crdt_states;
    let mut squash_attempted = false;
    let mut squash_applied = false;
    // Squashing changes the document epoch and replaces its CRDT history. Only
    // attempt it after this run was already fully quiet: a concurrent pull or
    // push would make the proposal stale, and a local edit racing the reset
    // would need the ordinary merge landing path.
    if snapshot.allow_squash
        && local_state.pending.is_empty()
        && remote_updates_applied == 0
        && pulled_changes.is_empty()
        && pushed.is_empty()
    {
        if let Some(proposal) = knotq_sync::build_squash_proposal(&crdt_docs, &local_state) {
            squash_attempted = true;
            match client.squash(&proposal.as_request(replica_id)) {
                Ok(response) => {
                    eprintln!(
                        "sync: squashed document {} history: {} -> {} bytes (epoch {})",
                        response.document,
                        proposal.bytes_before,
                        proposal.state_v1.len(),
                        response.epoch,
                    );
                    // Adopt the squashed state now (the server's changed-broadcast
                    // excludes this replica, so no nudge is coming): one more pull
                    // replaces the local document — with no pending edits it is
                    // exact. A failure here is harmless: the next regular sync
                    // adopts instead.
                    match batch_pull_and_apply(
                        transport,
                        &mut crdt_docs,
                        &mut local_state,
                        workspace.clone(),
                        replica_id,
                    ) {
                        Ok(adoption) => {
                            squash_applied = true;
                            workspace = adoption.workspace;
                            remote_updates_applied += adoption.remote_updates_applied;
                            merged_crdt_states = crdt_docs.document_states();
                            save_workspace(path, &workspace)?;
                            save_crdt_state(path, &merged_crdt_states)?;
                            save_local_sync_state(path, &local_state)?;
                        }
                        Err(err) => {
                            eprintln!("sync: post-squash adoption pull failed (next sync adopts): {err:#}");
                        }
                    }
                }
                Err(err) => eprintln!("sync: squash proposal declined: {err:#}"),
            }
        }
    }

    Ok(SyncRunResult {
        workspace,
        crdt_states: merged_crdt_states,
        pushed,
        queued_item_fields,
        remote_updates_applied,
        remaining_pending: local_state.pending.len(),
        local_workspace_changed: local_workspace_changed || repaired_workspace_changed,
        media_downloaded,
        notification_schedule,
        squash_attempted,
        squash_applied,
    })
}

/// A run that fails after saving its pull cursors never lands: the UI store keeps
/// the state it had before the pull, and the next run starts from that state
/// overlaid on the saved one. The server does not resend a document below its
/// cursor, so what the failed pull brought in would be lost for good (production
/// fuzz seed 6: a line another device moved stayed in its source scheme). Pull
/// those documents again instead — only those, so a transient failure never
/// forces a full re-download.
fn forget_pull_of_failed_run(
    path: &std::path::Path,
    local_state: &mut LocalSyncState,
    pulled: &[knotq_model::DocumentId],
) {
    for document in pulled {
        local_state.reset_pull_cursor(*document);
    }
    if let Err(err) = save_local_sync_state(path, local_state) {
        eprintln!("sync: could not persist re-pull cursors after a failed run: {err:#}");
    }
}

/// Queue a re-identified workspace document's full state as a pending push, so a
/// device adopting a different account's workspace id (sign-in / account switch)
/// uploads its local content to the new account. The bootstrap only force-pushes
/// documents the server has no base for, so a workspace the server already holds
/// would otherwise never receive this content; the server applies it as an
/// idempotent Yjs merge, unioning the local schemes in.
fn queue_reidentified_workspace_update(
    local_state: &mut LocalSyncState,
    replica_id: ReplicaId,
    workspace: &Workspace,
    update: CrdtDocumentUpdate,
) {
    let operation_id = OperationId::new();
    let local_sequence = local_state
        .pending
        .iter()
        .map(|edit| edit.local_sequence)
        .max()
        .unwrap_or(0)
        + 1;
    local_state.push_pending(PendingCrdtEdit {
        operation_id,
        workspace_id: workspace.id,
        replica_id,
        local_sequence,
        created_at: Utc::now(),
        document: update.document,
        kind: update.kind,
        update_v1: update.update_v1,
        touched_items: update.touched_items,
    });
}

fn queue_repair_crdt_updates(
    local_state: &mut LocalSyncState,
    workspace: &Workspace,
    replica_id: ReplicaId,
    crdt_docs: &mut WorkspaceCrdtDocuments,
) -> Result<()> {
    let outcome = crdt_docs.sync_changes(workspace, &WorkspaceCrdtChangeSet::default().workspace());
    for error in &outcome.errors {
        // A repair-encoding error for one document must not wedge the entire sync.
        // Log it and queue whatever updates did encode; the pull cursors still
        // persist, so the device keeps converging and retries the repair next sync
        // rather than failing every sync forever.
        eprintln!("sync: CRDT repair update skipped: {error}");
    }
    if outcome.updates.is_empty() {
        return Ok(());
    }
    let operation_id = OperationId::new();
    let local_sequence = local_state
        .pending
        .iter()
        .map(|edit| edit.local_sequence)
        .max()
        .unwrap_or(0)
        + 1;
    for update in outcome.updates {
        local_state.push_pending(PendingCrdtEdit {
            operation_id,
            workspace_id: workspace.id,
            replica_id,
            local_sequence,
            created_at: Utc::now(),
            document: update.document,
            kind: update.kind,
            update_v1: update.update_v1,
            touched_items: update.touched_items,
        });
    }
    Ok(())
}

pub(super) fn workspace_for_background_sync(
    path: &std::path::Path,
    current: Workspace,
) -> Workspace {
    let Ok(Some(mut full)) = load_workspace_with_options(path, WorkspaceLoadOptions::all()) else {
        return current;
    };
    if full.id != current.id {
        eprintln!(
            "sync full workspace load ignored: loaded workspace id {} does not match in-memory id {}",
            full.id, current.id
        );
        return current;
    }
    overlay_current_workspace_for_sync(&mut full, current);
    full
}

fn overlay_current_workspace_for_sync(full: &mut Workspace, current: Workspace) {
    // Everything but `schemes` comes from the in-memory workspace: the saved
    // files can be behind it whenever the save task has not run since an edit.
    // Only `schemes` merges, because memory holds just the loaded days.
    // Destructured exhaustively so a field added to `Workspace` has to be
    // decided here: the folder archive was once left out, and a folder archived
    // since the last save — in neither the tree nor the trash — was dropped from
    // the account for every device (production fuzz seed 10005).
    let Workspace {
        id,
        sync,
        root,
        folders,
        schemes,
        scheme_sync,
        folder_sync,
        daily_queue,
        recently_deleted,
        deleted_scheme_origins,
        recently_deleted_folders,
        deleted_folder_origins,
    } = current;
    full.id = id;
    full.sync = sync;
    full.root = root;
    full.folders = folders;
    full.scheme_sync = scheme_sync;
    full.folder_sync = folder_sync;
    full.daily_queue = daily_queue;
    full.recently_deleted = recently_deleted;
    full.deleted_scheme_origins = deleted_scheme_origins;
    full.recently_deleted_folders = recently_deleted_folders;
    full.deleted_folder_origins = deleted_folder_origins;
    for (scheme_id, scheme) in schemes {
        full.schemes.insert(scheme_id, scheme);
    }
    full.normalize_one_level_folders();
    full.normalize_item_markers();
    full.ensure_sync_metadata();
}

fn configure_local_state(
    local_state: &mut LocalSyncState,
    workspace_id: WorkspaceId,
    replica_id: ReplicaId,
    account: &SyncAccountSettings,
) {
    let server_workspace_id = sync_workspace_id(account, workspace_id);
    // Signing into a different account/server than the persisted cursors were built
    // against must not reuse the previous account's pull/push cursors: a stale cursor
    // silently skips pulling the new account's lower document sequences and makes the
    // bootstrap push a bare delta the new server has no base for (crdt_schema_invalid).
    // Reset them so the next sync re-pulls from zero and re-seeds full snapshots.
    if local_state.reset_for_account_change(server_workspace_id, &account.api_base) {
        eprintln!("sync: account/server changed since last sync — reset cursors for full re-pull");
    }
    local_state.workspace_id = Some(server_workspace_id);
    local_state.replica_id = Some(replica_id);
    local_state.server_url = Some(account.api_base.clone());
}

fn sync_workspace_id(account: &SyncAccountSettings, fallback: WorkspaceId) -> WorkspaceId {
    account
        .workspace_id
        .as_deref()
        .and_then(|workspace_id| workspace_id.parse().ok())
        .unwrap_or(fallback)
}

fn merge_pending(local_state: &mut LocalSyncState, pending: Vec<PendingCrdtEdit>) {
    for edit in pending {
        if !local_state.pending.iter().any(|existing| {
            existing.operation_id == edit.operation_id
                && existing.document == edit.document
                && existing.local_sequence == edit.local_sequence
        }) {
            local_state.push_pending(edit);
        }
    }
}

/// Keep expected orphan traffic from drowning out actionable sync failures.
/// Orphans are normal after a remote deletion or while an index/content pair
/// is converging, but a materialization gap is not: it means the server sent a
/// document that the local workspace index references but the client could not
/// turn into a live CRDT document. Preserve the detailed error for every
/// non-benign skip and emit a bounded sample for the expected case.
fn log_skipped_documents(skipped: &[SkippedDocument]) {
    let orphan_documents: Vec<String> = skipped
        .iter()
        .filter(|skipped| skipped.unknown_scheme_document)
        .map(|skipped| skipped.document.to_string())
        .collect();
    if !orphan_documents.is_empty() {
        let sample = orphan_documents
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if orphan_documents.len() > 3 {
            ", …"
        } else {
            ""
        };
        eprintln!(
            "sync: ignored {} orphan document(s) (no workspace index entry); sample={}{}",
            orphan_documents.len(),
            sample,
            suffix
        );
    }

    for skipped in skipped
        .iter()
        .filter(|skipped| !skipped.unknown_scheme_document)
    {
        let category = if skipped.deferred {
            "materialization gap"
        } else {
            "skipped document"
        };
        eprintln!(
            "sync: {category} {} ({:?}): {}",
            skipped.document, skipped.kind, skipped.reason
        );
    }
}

#[cfg(test)]
mod configure_local_state_tests {
    use super::configure_local_state;
    use chrono::Utc;
    use knotq_model::{DocumentId, ReplicaId, SyncAccountSettings, SyncDocumentKind, WorkspaceId};
    use knotq_sync::LocalSyncState;

    fn account(api_base: &str) -> SyncAccountSettings {
        SyncAccountSettings {
            api_base: api_base.to_string(),
            user_id: "user".to_string(),
            session_id: None,
            // None so `sync_workspace_id` falls back to the passed workspace id,
            // letting the test drive the account identity via the function argument.
            workspace_id: None,
            email: "user@example.com".to_string(),
            supports_sync: true,
            bearer_token: "token".to_string(),
            expires_at: Utc::now(),
            refresh_token: None,
            refresh_expires_at: None,
            account_status: None,
        }
    }

    fn state_with_cursor(workspace_id: WorkspaceId, server_url: &str) -> LocalSyncState {
        let mut state = LocalSyncState {
            workspace_id: Some(workspace_id),
            replica_id: Some(ReplicaId::new()),
            server_url: Some(server_url.to_string()),
            ..LocalSyncState::default()
        };
        state.mark_pulled(DocumentId::new(), SyncDocumentKind::Scheme, 5, 0);
        state
    }

    #[test]
    fn resets_cursors_when_signing_into_a_different_account() {
        let account_a = WorkspaceId::new();
        let mut state = state_with_cursor(account_a, "https://a.example.com");
        assert_eq!(state.document_cursors.len(), 1);

        let account_b = WorkspaceId::new();
        configure_local_state(
            &mut state,
            account_b,
            ReplicaId::new(),
            &account("https://b.example.com"),
        );

        assert!(
            state.document_cursors.is_empty(),
            "signing into account B must clear account A's cursors"
        );
        assert_eq!(state.workspace_id, Some(account_b));
        assert_eq!(state.server_url.as_deref(), Some("https://b.example.com"));
    }

    #[test]
    fn keeps_cursors_when_account_and_server_are_unchanged() {
        let account_a = WorkspaceId::new();
        let mut state = state_with_cursor(account_a, "https://a.example.com");

        configure_local_state(
            &mut state,
            account_a,
            ReplicaId::new(),
            &account("https://a.example.com"),
        );

        assert_eq!(
            state.document_cursors.len(),
            1,
            "a normal re-sync of the same account must not discard cursors"
        );
    }
}

#[cfg(test)]
mod skipped_document_logging_tests {
    use super::log_skipped_documents;
    use knotq_model::{DocumentId, SyncDocumentKind};
    use knotq_sync::SkippedDocument;

    // This test is intentionally a smoke test for the diagnostic partitioning:
    // it exercises the same values that caused the desktop runtime's noisy
    // orphan output, while ensuring non-benign gaps still take the detailed
    // branch without panicking. Log text is verified manually in the runtime
    // smoke loop because stderr capture is platform-specific in this crate.
    #[test]
    fn partitions_orphans_and_materialization_gaps_without_panicking() {
        let skipped = vec![
            SkippedDocument {
                document: DocumentId::new(),
                kind: SyncDocumentKind::Scheme,
                unknown_scheme_document: true,
                deferred: false,
                reason: "deleted remotely".to_string(),
            },
            SkippedDocument {
                document: DocumentId::new(),
                kind: SyncDocumentKind::Scheme,
                unknown_scheme_document: false,
                deferred: true,
                reason: "pulled but did not materialize".to_string(),
            },
        ];

        log_skipped_documents(&skipped);
    }
}
