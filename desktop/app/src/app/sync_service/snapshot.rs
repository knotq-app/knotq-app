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
    WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments,
};

use super::http::normalize_api_base;
use super::media::{download_missing_media_assets, upload_local_media_assets};
use super::{SyncHttpClient, SyncRunResult, SyncSnapshot};

pub(super) fn sync_snapshot(snapshot: SyncSnapshot) -> Result<SyncRunResult> {
    let path = workspace_path();
    let mut workspace = workspace_for_background_sync(&path, snapshot.workspace);
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
    let local_workspace_changed =
        workspace.canonicalize_personal_sync_identity(server_workspace_id);
    workspace.ensure_sync_metadata();

    let mut local_state = load_local_sync_state(&path).unwrap_or_default();
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

    let client = SyncHttpClient {
        api_base: normalize_api_base(&snapshot.account.api_base)?,
        bearer_token: snapshot.account.bearer_token.clone(),
    };
    // Batched pull/push prefer the live WebSocket and fall back to `client` (HTTP)
    // when the socket is down. Media transfer always uses `client` (HTTP) directly.
    let ws_sync = snapshot.ws_sync.clone();
    let transport = super::ws_transport::FallbackTransport::new(ws_sync.as_deref(), &client);
    // Restore the long-lived CRDT documents from disk and overlay the UI store's
    // latest states (the `snapshot`), so the sync's CRDT carries this device's stable
    // deterministic identity plus its newest local edits — never rebuilt from plain
    // data. Disk fills documents the in-memory store doesn't hold (e.g. archived /
    // off-screen Daily Queue schemes loaded by `workspace_for_background_sync`).
    let mut crdt_states = load_crdt_state(&path).unwrap_or_default();
    crdt_states.extend(snapshot.crdt_states);
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
                    update_v1: state,
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
    if account_switched {
        // Force re-seed this device's scheme content to the new account. The bootstrap
        // below only re-seeds documents the new server LACKS, so a scheme the new
        // account already holds from another origin would otherwise never receive this
        // device's content (the cross-account content gap). Full snapshots union
        // idempotently; deterministic item creation dedupes items.
        queue_account_switch_reseed(&mut local_state, &crdt_docs, &workspace, snapshot.replica_id);
    }

    let mut pushed = Vec::new();

    // One batched pull syncs the whole workspace: the server returns the current
    // merged state of every document whose seq advanced past our cursor (and any
    // document created on another device). Applying it materializes the merged
    // workspace; the engine applies the workspace index before scheme content so
    // newly discovered schemes route correctly.
    let pull = batch_pull_and_apply(
        &transport,
        &mut crdt_docs,
        &mut local_state,
        workspace,
        snapshot.replica_id,
    )?;
    // Log skipped documents (per-document errors that did not block the pull).
    for skipped in &pull.skipped {
        if skipped.unknown_scheme_document {
            eprintln!(
                "sync: ignored orphan document {} (no workspace index entry)",
                skipped.document
            );
        } else {
            eprintln!(
                "sync: skipped document {}: {}",
                skipped.document, skipped.reason
            );
        }
    }
    let mut workspace = pull.workspace;
    let remote_updates_applied = pull.remote_updates_applied;
    let mut repaired_workspace_changed =
        workspace.canonicalize_personal_sync_identity(server_workspace_id);
    repaired_workspace_changed |= workspace.normalize_one_level_folders();
    repaired_workspace_changed |= workspace.normalize_item_markers();
    if repaired_workspace_changed {
        queue_repair_crdt_updates(
            &mut local_state,
            &workspace,
            snapshot.replica_id,
            &mut crdt_docs,
        )?;
    }

    upload_local_media_assets(&client, &mut local_state, &workspace, &pull.remote_latest)?;
    let mut media_downloaded = download_missing_media_assets(&client, &workspace)?;

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
        || repaired_workspace_changed
        || !healed_documents.is_empty()
    {
        save_workspace(&path, &workspace)?;
        save_crdt_state(&path, &merged_crdt_states)?;
    }

    // Persist pull cursors, dropped orphans, and per-document push acks even if the
    // push below fails partway, so a transient push error never forces the next
    // sync to re-download every document from sequence zero. The merged workspace
    // above is already durable, so the cursor never runs ahead of it.
    let push_result = batch_push_pending(
        &transport,
        &mut local_state,
        replica_id,
        &notification_schedule,
        &mut pushed,
        &mut crdt_docs,
        &workspace,
    );
    save_local_sync_state(&path, &local_state)?;
    // The push's own self-heal may have repopulated a schema-less document after
    // the capture above; persist the healed state so this device's future diffs
    // share its identity instead of re-minting the same clientID from clock zero.
    let merged_crdt_states = {
        let post_push_states = crdt_docs.document_states();
        if post_push_states != merged_crdt_states {
            save_crdt_state(&path, &post_push_states)?;
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
    upload_local_media_assets(&client, &mut local_state, &workspace, &media_remote_latest)?;
    save_local_sync_state(&path, &local_state)?;
    media_downloaded |= download_missing_media_assets(&client, &workspace)?;

    Ok(SyncRunResult {
        workspace,
        crdt_states: merged_crdt_states,
        pushed,
        remote_updates_applied,
        remaining_pending: local_state.pending.len(),
        local_workspace_changed: local_workspace_changed || repaired_workspace_changed,
        media_downloaded,
        notification_schedule,
    })
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
        });
    }
    Ok(())
}

pub(super) fn workspace_for_background_sync(path: &std::path::Path, current: Workspace) -> Workspace {
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
    full.id = current.id;
    full.sync = current.sync;
    full.root = current.root;
    full.folders = current.folders;
    full.scheme_sync = current.scheme_sync;
    full.folder_sync = current.folder_sync;
    full.daily_queue = current.daily_queue;
    full.recently_deleted = current.recently_deleted;
    full.deleted_scheme_origins = current.deleted_scheme_origins;
    for (scheme_id, scheme) in current.schemes {
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
        eprintln!(
            "sync: account/server changed since last sync — reset cursors for full re-pull"
        );
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

#[cfg(test)]
mod configure_local_state_tests {
    use super::configure_local_state;
    use chrono::Utc;
    use knotq_model::{
        DocumentId, ReplicaId, SyncAccountSettings, SyncDocumentKind, WorkspaceId,
    };
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
        state.mark_pulled(DocumentId::new(), SyncDocumentKind::Scheme, 5);
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
