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
    // Keep the pre-sign-in content available for the first-join workspace-index
    // re-root below. Its random local WorkspaceId is part of the deterministic
    // population input, so merely relabelling the document would leave this
    // device's starter history incompatible with the account's canonical one.
    let pre_canonical_workspace = workspace.clone();
    // Capture the workspace document's id before adopting the account's canonical
    // identity, so an account switch can carry its content to the new id below.
    let previous_workspace_document_id = workspace.sync.id;
    let (_local_workspace_repair_needed, local_workspace_changed) =
        workspace.canonicalize_personal_sync_identity_with_change(server_workspace_id);
    workspace.ensure_sync_metadata();

    let mut local_state = load_local_sync_state(path).unwrap_or_default();
    // A never-synced install is joining this account, not switching away from
    // another one. Its starter scheme snapshots can contain items the account
    // already deleted, so reseeding every existing scheme after the pull would
    // resurrect those items under fresh CRDT identities. The normal bootstrap
    // below still seeds genuinely new local schemes and queued local edits are
    // merged normally; exhaustive scheme reseeding is only for a real account
    // switch, where carrying this device's prior account content is required.
    let had_prior_sync_identity = local_state.workspace_id.is_some()
        || local_state
            .server_url
            .as_deref()
            .is_some_and(|url| !url.is_empty());
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
    local_state
        .recent_item_edits
        .extend(snapshot.recent_item_edits);
    local_state
        .recent_folder_edits
        .extend(snapshot.recent_folder_edits);
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
        let source = if crdt_states.contains_key(&previous_workspace_document_id) {
            Some(previous_workspace_document_id)
        } else if had_prior_sync_identity && !crdt_states.contains_key(&workspace.sync.id) {
            // Neither id has a state, so `from_states` is about to build this
            // account's index EMPTY and the pull will materialize over nothing.
            // Look the index up by shape instead — see the function's comment.
            stale_workspace_index_by_shape(&crdt_states, workspace.sync.id, &workspace)
        } else {
            None
        };
        source
            .and_then(|document| crdt_states.remove(&document))
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
    if account_switched && !had_prior_sync_identity {
        let mut canonical_base = pre_canonical_workspace.clone();
        canonical_base.canonicalize_personal_sync_identity_with_change(server_workspace_id);
        canonical_base.ensure_sync_metadata();
        // The local CRDT was populated before sign-in. Rebuild that population
        // under the account's canonical identity, then re-express the current
        // plain workspace as an ordinary edit on top. This preserves offline and
        // in-flight local edits while making untouched starter content byte-
        // identical to every other first joiner, so Yjs can deduplicate it.
        crdt_docs.repopulate_workspace_canonically(
            &canonical_base,
            &workspace,
            workspace.sync.id,
        )?;
        let update = crdt_docs
            .full_snapshot_updates_for_documents(&std::collections::HashSet::from([workspace
                .sync
                .id]))
            .updates
            .into_iter()
            .next();
        if let Some(update) = update {
            queue_reidentified_workspace_update(
                &mut local_state,
                snapshot.replica_id,
                &workspace,
                update,
            );
        }
    }
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
    // What this device held going in. A pull merges; it should not subtract, so
    // a scheme here that is missing afterwards is one the merge could not
    // account for — the shape that costs a whole page and leaves no removal in
    // any index write to explain it.
    let schemes_before_pull = held_schemes(&workspace, &local_state);
    {
        // A scheme the plain workspace has whose content document this replica
        // does not hold at all. The pull is about to materialize from the
        // documents, so such a scheme cannot survive it — and on a device that
        // has switched accounts the projection law is excused (TODO 0i-b), so
        // nothing upstream reports the gap either.
        let known = crdt_docs.known_document_ids();
        let mut documentless: Vec<String> = workspace
            .schemes
            .keys()
            .filter(|id| {
                workspace
                    .scheme_sync
                    .get(id)
                    .is_none_or(|meta| !known.contains(&meta.id))
            })
            .map(|id| id.to_string())
            .collect();
        if !documentless.is_empty() {
            documentless.sort();
            eprintln!(
                "sync: {} scheme(s) have no CRDT document going into the pull: {}",
                documentless.len(),
                documentless.join(", ")
            );
        }
    }
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
    let restored_schemes = restore_unpublished_schemes_dropped_by_pull(
        &mut workspace,
        &schemes_before_pull,
        &crdt_docs,
        &local_state,
        &pull.remote_latest,
    );
    let remote_updates_applied = pull.remote_updates_applied;
    let locally_repaired_documents = pull.locally_repaired_documents;
    let (repaired_identity, repaired_identity_changed) =
        workspace.canonicalize_personal_sync_identity_with_change(server_workspace_id);
    let repaired_folders = workspace.normalize_one_level_folders();
    let repaired_marker_schemes = workspace.repair_item_markers();
    let repaired_markers = !repaired_marker_schemes.is_empty();
    let restored_any = !restored_schemes.is_empty();
    let repaired_workspace_changed =
        repaired_identity || repaired_folders || repaired_markers || restored_any;
    let repaired_workspace_persist_changed =
        repaired_identity_changed || repaired_folders || repaired_markers || restored_any;
    if repaired_workspace_changed {
        // A restored scheme goes in beside the marker repairs: the index write
        // is what re-adds its node entry, and writing its content back is a
        // no-op when the body came out of the document it is written to.
        let mut repaired_scheme_content = repaired_marker_schemes.clone();
        repaired_scheme_content.extend(restored_schemes.iter().copied());
        queue_repair_crdt_updates(
            &mut local_state,
            &workspace,
            snapshot.replica_id,
            &mut crdt_docs,
            &repaired_scheme_content,
        )?;
    }
    if account_switched && had_prior_sync_identity {
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
    // The paired workspace and CRDT state are durable by this point. A
    // relaunch can therefore no longer need the pre-save plain-workspace base,
    // even if the network push itself failed and the pending edits remain.
    local_state.workspace_save_recovery = None;
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

    // A successful push changes the local CRDT documents even when the pull
    // before it was empty.  The workspace materialized above therefore cannot
    // be returned as-is: it predates the accepted local edits, while the CRDT
    // states we hand to landing already contain them.  Keep the two halves of a
    // sync result self-consistent by materializing once more from those final
    // documents.  Without this boundary, a crash/relaunch recovery or an
    // in-flight landing can adopt a pre-push workspace index beside post-push
    // CRDT bytes and silently undo a carry-over, archive/restore, or metadata
    // edit on the next sync.
    let pushed_documents: std::collections::HashSet<_> =
        pushed.iter().map(|document| document.document).collect();
    let pushed_scheme_documents: std::collections::HashSet<_> = workspace
        .scheme_sync
        .iter()
        .filter_map(|(scheme, metadata)| pushed_documents.contains(&metadata.id).then_some(*scheme))
        .collect();
    let mut post_push_workspace = crdt_docs
        .materialized_workspace_repair(&workspace, &|scheme| {
            pushed_scheme_documents.contains(scheme)
        })?;
    // Materialization does not normalize (TODO 0j: a normalizing read makes
    // every sync rewrite the line), so a workspace that comes back out of the
    // documents has to be put into the model's normal form before it becomes
    // the visible one — otherwise the device shows a combination the model
    // forbids, the next snapshot quietly normalizes the copy it writes into
    // the documents, and the two halves disagree for good (single-account
    // fuzz seed 10024: a date left on a line that is no longer a checkbox).
    let _ = post_push_workspace.normalize_item_markers();
    if post_push_workspace != workspace {
        workspace = post_push_workspace;
        save_workspace(path, &workspace)?;
    }
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
                            // Same rule as the post-push materialization
                            // above: what comes out of the documents is
                            // normalized before it becomes the visible half.
                            let _ = workspace.normalize_item_markers();
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

/// Enough of a scheme this device holds going into a pull to put it back.
///
/// Deliberately not a clone of the pre-pull workspace. The items dominate the
/// cost of cloning one, and only a scheme that could actually need restoring
/// carries its body here (see `items`); for every other scheme this is a name,
/// a binding and a position.
struct HeldScheme {
    sync: knotq_model::SyncDocumentMeta,
    name: String,
    color_index: u8,
    gsync: bool,
    source: knotq_model::SchemeSource,
    /// Where the folder tree had it, so a restore puts it back rather than
    /// dropping it at the root.
    placement: Option<(knotq_model::FolderId, usize)>,
    archived: bool,
    /// Whether the Daily Queue binds this scheme to a date.
    daily: bool,
    /// The body, captured only for a scheme that could need putting back.
    ///
    /// Applying a pull PRUNES the live CRDT document of a scheme the merged
    /// index neither materializes nor binds, so by the time the rescue below
    /// runs there is nothing left to read the items out of. They have to be
    /// taken before the pull — but cloning every scheme's items on every sync
    /// is the most expensive thing a workspace can be asked to do, so this is
    /// filled in only for a document the server has never sent us anything for
    /// that still has local edits waiting. That is a freshly created page and
    /// almost always nothing at all.
    items: Option<Vec<knotq_model::Item>>,
}

fn held_schemes(
    workspace: &Workspace,
    local_state: &LocalSyncState,
) -> std::collections::HashMap<knotq_model::SchemeId, HeldScheme> {
    let mut placements: std::collections::HashMap<
        knotq_model::SchemeId,
        (knotq_model::FolderId, usize),
    > = std::collections::HashMap::new();
    for (folder_id, folder) in &workspace.folders {
        for (position, child) in folder.children.iter().enumerate() {
            if let knotq_model::NodeRef::Scheme(scheme) = child {
                placements.insert(*scheme, (*folder_id, position));
            }
        }
    }
    let archived: std::collections::HashSet<knotq_model::SchemeId> =
        workspace.recently_deleted.iter().copied().collect();
    let daily: std::collections::HashSet<knotq_model::SchemeId> =
        workspace.daily_queue.values().copied().collect();
    workspace
        .schemes
        .iter()
        .filter_map(|(id, scheme)| {
            let sync = workspace.scheme_sync.get(id)?.clone();
            // A cheap superset of the authoritative `remote_latest` test the
            // restore applies after the pull: a document the server has never
            // heard of cannot have a pull cursor above zero.
            let never_pulled = local_state
                .document_cursors
                .get(&sync.id)
                .is_none_or(|cursor| cursor.last_pulled_sequence == 0);
            let at_risk = never_pulled && local_state.has_pending_for_document(sync.id);
            Some((
                *id,
                HeldScheme {
                    sync,
                    name: scheme.name.clone(),
                    color_index: scheme.color_index,
                    gsync: scheme.gsync,
                    source: scheme.source.clone(),
                    placement: placements.get(id).copied(),
                    archived: archived.contains(id),
                    daily: daily.contains(id),
                    items: at_risk.then(|| scheme.items.clone()),
                },
            ))
        })
        .collect()
}

/// Put back a scheme this device created and has never published that the pull
/// materialized away, and report every other scheme the pull subtracted.
///
/// A pull merges; it should not subtract. It materializes from the workspace
/// INDEX document, and the index write happens *after* the pull, from the
/// pull's own result — so a scheme created while a sync was in flight is not in
/// the index the next pull materializes from, and that pull drops the whole
/// page (production fuzz chaos seed 140, TODO 0t). `retained_loaded_schemes`
/// in the CRDT layer already rescues this shape, but only for a scheme whose
/// `scheme_sync` binding survives in the MERGED index; here the index has never
/// heard of the scheme at all.
///
/// The question that separates this from a scheme the account deleted remotely
/// is "did this device ever publish it?", which the plain workspace cannot
/// answer and the pull can. Both halves of that are required:
///
/// - the server has no sequence for the document (`remote_latest`), so no other
///   device can ever have seen it, let alone deleted it; and
/// - this device still has pending edits for the document, so the creation is
///   demonstrably unpublished local work rather than an old page whose cursors
///   happen to have been reset.
///
/// The second half is not belt-and-braces. `remote_latest` falls back to local
/// cursors when a response carries no `known_documents`, and an account switch
/// resets those cursors — on its own, the first test would then read every
/// scheme of the account being left as "never published" and resurrect the lot.
/// That is the shape recorded in TODO 0t as taking the gate from 1 failing seed
/// to 32.
fn restore_unpublished_schemes_dropped_by_pull(
    workspace: &mut Workspace,
    held: &std::collections::HashMap<knotq_model::SchemeId, HeldScheme>,
    crdt_docs: &WorkspaceCrdtDocuments,
    local_state: &LocalSyncState,
    remote_latest: &std::collections::HashMap<knotq_model::DocumentId, u64>,
) -> std::collections::HashSet<knotq_model::SchemeId> {
    let mut dropped: Vec<(&knotq_model::SchemeId, &HeldScheme)> = held
        .iter()
        .filter(|(id, _)| !workspace.schemes.contains_key(id))
        .collect();
    if dropped.is_empty() {
        return std::collections::HashSet::new();
    }
    // A HashMap walk is unordered and this decides where lines land in a
    // folder's children, so fix an order every replica agrees on.
    dropped.sort_by_key(|(id, _)| id.to_string());

    let daily_now: std::collections::HashSet<knotq_model::SchemeId> =
        workspace.daily_queue.values().copied().collect();
    let mut restored = std::collections::HashSet::new();
    let mut reported: Vec<String> = Vec::new();
    for (scheme_id, scheme) in dropped {
        let published = remote_latest.get(&scheme.sync.id).copied().unwrap_or(0) != 0;
        let unpushed = local_state.has_pending_for_document(scheme.sync.id);
        // An archived scheme is detached from the folder tree and its archive
        // entry is retained by normalization on structural grounds, so putting
        // one back is a different repair than this. None has been observed;
        // report it rather than guess.
        // A Daily page absent from the materialized workspace is not lost and
        // must not be rebuilt here. A day outside the loaded window is
        // deliberately left out of the plain workspace, and putting one back
        // breaks the projection law from the other side — the materialized half
        // then holds a page the visible half does not (the same trap documented
        // on `retained_loaded_schemes` for chaos 108 / single-account 10214, and
        // reached by this rescue on chaos 127). The client brings the day back
        // from its binding through `ensure_daily_queue` when it needs it.
        let daily = scheme.daily || daily_now.contains(scheme_id);
        let restorable = !published && unpushed && !scheme.archived && !daily;
        let Some(items) = crdt_docs
            .materialized_scheme_items(*scheme_id)
            .or_else(|| scheme.items.clone())
            .filter(|_| restorable)
        else {
            reported.push(format!(
                "{scheme_id} (published={published} unpushed={unpushed} \
                 archived={} daily={daily})",
                scheme.archived
            ));
            continue;
        };
        workspace.schemes.insert(
            *scheme_id,
            knotq_model::Scheme {
                id: *scheme_id,
                name: scheme.name.clone(),
                color_index: scheme.color_index,
                gsync: scheme.gsync,
                source: scheme.source.clone(),
                items,
            },
        );
        workspace
            .scheme_sync
            .entry(*scheme_id)
            .or_insert_with(|| scheme.sync.clone());
        // Put it back where the user had it when that folder survived the pull.
        // Otherwise leave it unplaced: `normalize_one_level_folders` runs next
        // and re-homes a scheme the folder tree does not mention under the root,
        // which is the same choice it makes for every other stranded node.
        if let Some((folder_id, position)) = scheme.placement {
            if let Some(folder) = workspace.folders.get_mut(&folder_id) {
                let child = knotq_model::NodeRef::Scheme(*scheme_id);
                if !folder.children.contains(&child) {
                    let position = position.min(folder.children.len());
                    folder.children.insert(position, child);
                }
            }
        }
        restored.insert(*scheme_id);
    }

    if !restored.is_empty() {
        let mut names: Vec<String> = restored.iter().map(|id| id.to_string()).collect();
        names.sort();
        eprintln!(
            "sync: restored {} unpublished scheme(s) the pull dropped: {}",
            names.len(),
            names.join(", ")
        );
    }
    if !reported.is_empty() {
        eprintln!(
            "sync: the pull dropped {} scheme(s) this device held: {}",
            reported.len(),
            reported.join(", ")
        );
    }
    restored
}

fn queue_repair_crdt_updates(
    local_state: &mut LocalSyncState,
    workspace: &Workspace,
    replica_id: ReplicaId,
    crdt_docs: &mut WorkspaceCrdtDocuments,
    repaired_scheme_content: &std::collections::HashSet<knotq_model::SchemeId>,
) -> Result<()> {
    // The identity/folder repairs above rewrite the index, but a marker repair
    // rewrites item content — and a repair that reaches only the plain
    // workspace leaves the two halves of this device describing different
    // things. The very next pull reads that difference as a local edit and
    // re-asserts the stale plain value over the document's merged one, with no
    // other device involved. So the schemes whose items were repaired go into
    // the change set alongside the index.
    let mut changes = WorkspaceCrdtChangeSet::default().workspace();
    changes
        .schemes
        .extend(repaired_scheme_content.iter().copied());
    let outcome = crdt_docs.sync_changes(workspace, &changes);
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
        // Report it, but do NOT fall back to `current`. `current` holds only the
        // loaded window, so every unloaded scheme — every Daily page outside the
        // window, every scheme this session has not opened — would look absent,
        // and the index write below publishes an absence as an authoritative
        // deletion for the whole account. Production fuzz chaos seed 140: three
        // schemes, a Daily page, its item and its queue binding left device 0 in
        // a single step, with nothing deleted anywhere.
        //
        // A mismatch is not evidence of a foreign data directory. `current` was
        // itself loaded from `path`; the ids differ because the in-memory
        // workspace adopted a canonical sync identity (sign-in, account switch)
        // that the save task has not written out yet. The overlay already
        // resolves exactly that: it takes the in-memory identity wholesale
        // (`full.id = id`, and every other field but `schemes`) and keeps only
        // the disk's copy of the schemes memory does not hold.
        eprintln!(
            "sync full workspace load: stored workspace id {} is behind the in-memory id {}; keeping its unloaded schemes",
            full.id, current.id
        );
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
    let _ = full.normalize_item_markers();
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

/// The id this device's workspace-index document is actually sitting under,
/// when the plain workspace names an id that nothing has ever written.
///
/// `WorkspaceCrdtDocuments::from_states` keys the index by `workspace.sync.id`.
/// With no state under that id it builds the index **empty**, the pull
/// materializes the account's index over nothing, and every scheme this device
/// holds that the account does not is dropped — then published to the account
/// as an authoritative deletion by the next index write.
///
/// The caller only reaches this when the current id has no state *and* the id
/// the switch moved away from has none either, so whenever it returns `Some`
/// the alternative was provably an empty index: a wrong answer here cannot be
/// worse than no answer.
///
/// That situation means the workspace's identity moved without the CRDT
/// following it (production fuzz chaos seed 140: device 0's workspace id became
/// a freshly minted UUID between two syncs while its index document stayed
/// under the account's id, so the account switch a few steps later looked for
/// the index under an id nothing had ever written, and the device lost three
/// schemes, a Daily page, its item and its queue binding). The index is then
/// found by shape rather than by id: a persisted state the workspace index does
/// not address, which decodes as an index rather than as scheme content.
fn stale_workspace_index_by_shape(
    crdt_states: &std::collections::HashMap<knotq_model::DocumentId, std::sync::Arc<[u8]>>,
    current: knotq_model::DocumentId,
    workspace: &Workspace,
) -> Option<knotq_model::DocumentId> {
    let addressed: std::collections::HashSet<knotq_model::DocumentId> = workspace
        .scheme_sync
        .values()
        .map(|metadata| metadata.id)
        .chain(
            workspace
                .daily_queue
                .keys()
                .map(|date| knotq_model::daily_queue_document_id(*date)),
        )
        .collect();
    // Deterministic: a directory that somehow holds two stale indexes must not
    // pick between them by hash order.
    let mut candidates: Vec<knotq_model::DocumentId> = crdt_states
        .iter()
        .filter(|(document, _)| **document != current && !addressed.contains(document))
        .filter(|(_, state)| knotq_sync::state_is_workspace_index(state))
        .map(|(document, _)| *document)
        .collect();
    candidates.sort();
    candidates.into_iter().next()
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
