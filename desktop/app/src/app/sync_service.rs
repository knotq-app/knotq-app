use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use async_channel::Receiver;
use chrono::Utc;
use futures::{pin_mut, select, FutureExt};
use gpui::{Context, Task};
use knotq_model::{
    DocumentId, ImageAssetFormat, ImageInline, Inline, Item, OperationId, ReplicaId,
    SyncAccountSettings, SyncAccountStatus, Workspace, WorkspaceId,
};
use knotq_storage_json::{
    image_asset_path, load_crdt_state, load_local_sync_state, load_workspace_with_options,
    save_crdt_state, save_local_sync_state, save_workspace, workspace_path, WorkspaceLoadOptions,
};
use knotq_sync::{
    batch_pull_and_apply, batch_push_pending, queue_workspace_bootstrap_updates, BatchPullRequest,
    BatchPullResponse, BatchPushRequest, BatchPushResponse, ErrorResponse, LocalSyncState,
    NotificationScheduleSnapshot, PendingCrdtEdit, PushedDocument, SyncTransport,
    WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments, MAX_SYNC_MEDIA_BYTES,
};
use sha2::{Digest, Sha256};
use std::fmt;

use super::sync_auth::{refresh_sync_backend, RefreshError};
use super::{KnotQApp, SyncAuthStatus, SyncRunStatus, View};

// ── Sync scheduling constants ─────────────────────────────────────────────
//
// Signal debounces (how long to wait after a signal before running):
//   Immediate  → 2 s  (sign-in, manual "Sync now", window activation)
//   LocalChange → 30 s (every local edit; timer runs from the *first* change in
//                        a burst so rapid typing doesn't postpone the run forever)
//
// Poll cadences (timer used when no signal has fired):
//   Pending edits AND server not rejecting → 30 s  (retry-after-offline)
//   Offline (last run failed at transport) → 20 min (back off while unreachable)
//   Window active                          → 2 min  (foreground poll)
//   Window inactive                        → 30 min (background poll)
//
// Server-rejection exception: when the server is online but refuses pushes
// (non-transport error), retrying every 30 s would hammer the backend; fall
// back to the foreground/background cadence instead.
const SYNC_DEBOUNCE: StdDuration = StdDuration::from_secs(2);
const SYNC_LOCAL_CHANGE_DEBOUNCE: StdDuration = StdDuration::from_secs(30);
const SYNC_PENDING_RETRY: StdDuration = StdDuration::from_secs(30);
const SYNC_POLL_FOREGROUND: StdDuration = StdDuration::from_secs(120);
const SYNC_POLL_BACKGROUND: StdDuration = StdDuration::from_secs(30 * 60);
const SYNC_POLL_OFFLINE: StdDuration = StdDuration::from_secs(20 * 60);
// Refresh the access token this many seconds before it expires, so a sync run
// never starts with a token that could lapse mid-flight.
const ACCESS_REFRESH_SKEW_SECS: i64 = 120;

/// Payload sent over the sync signal channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SyncSignal {
    /// Run after a 2 s debounce: sign-in completion, manual "Sync now", window
    /// activation.
    Immediate,
    /// Run after a 30 s debounce (from the first change in a burst): every local
    /// workspace edit.
    LocalChange,
}

/// Marker error attached to transport-level failures so the scheduler can
/// detect "offline" vs "server rejection" without parsing error strings.
#[derive(Debug)]
struct SyncNetworkUnreachable;

impl fmt::Display for SyncNetworkUnreachable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("network unreachable")
    }
}

impl std::error::Error for SyncNetworkUnreachable {}

#[derive(Clone)]
struct SyncSnapshot {
    workspace: Workspace,
    account: SyncAccountSettings,
    replica_id: ReplicaId,
    pending: Vec<PendingCrdtEdit>,
    /// This device's current CRDT document state, so the background sync seeds its
    /// CRDT from the UI store's latest local edits (with the same stable identity)
    /// rather than from a possibly-staler on-disk copy.
    crdt_states: HashMap<DocumentId, Vec<u8>>,
    notification_schedule: NotificationScheduleSnapshot,
}

struct SyncRunResult {
    workspace: Workspace,
    /// The merged CRDT document state after applying remote updates, handed back so
    /// the UI store adopts the canonical merged identity (never rebuilt from plain
    /// data).
    crdt_states: HashMap<DocumentId, Vec<u8>>,
    pushed: Vec<PushedDocument>,
    remote_updates_applied: usize,
    remaining_pending: usize,
    local_workspace_changed: bool,
    media_downloaded: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SyncMediaAsset {
    document: DocumentId,
    asset: uuid::Uuid,
    format: ImageAssetFormat,
}

impl SyncMediaAsset {
    fn image_name(self) -> String {
        format!("{}.{}", self.asset, self.format.extension())
    }
}

struct SyncHttpClient {
    api_base: String,
    bearer_token: String,
}

pub(crate) fn spawn_sync_task(
    sync_rx: Receiver<SyncSignal>,
    cx: &mut Context<KnotQApp>,
) -> Task<()> {
    cx.spawn(
        async move |weak: gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp| {
            // Run once at startup (immediate, no debounce).
            record_poll_at(&weak, cx);
            run_sync_once(&weak, cx).await;
            loop {
                // Choose the timer interval based on current app state.
                let interval = poll_interval(&weak, cx);
                let timer = cx.background_executor().timer(interval).fuse();
                let signal = sync_rx.recv().fuse();
                pin_mut!(timer, signal);
                let mut received_signal: Option<SyncSignal> = None;
                select! {
                    _ = timer => {}
                    result = signal => {
                        match result {
                            Ok(sig) => received_signal = Some(sig),
                            Err(_) => break,
                        }
                    }
                }

                if let Some(first_signal) = received_signal {
                    // Debounce: LocalChange waits 30 s from the *first* signal;
                    // Immediate waits 2 s. During the wait keep listening — an
                    // Immediate mid-wait shortens the deadline to now+2 s (only
                    // if that is sooner than the current deadline), further
                    // LocalChanges do not extend the 30 s deadline.
                    let debounce = match first_signal {
                        SyncSignal::Immediate => SYNC_DEBOUNCE,
                        SyncSignal::LocalChange => SYNC_LOCAL_CHANGE_DEBOUNCE,
                    };
                    let mut debounce_end = std::time::Instant::now() + debounce;
                    loop {
                        let remaining = debounce_end
                            .checked_duration_since(std::time::Instant::now())
                            .unwrap_or(StdDuration::ZERO);
                        if remaining.is_zero() {
                            break;
                        }
                        let timer = cx.background_executor().timer(remaining).fuse();
                        let signal = sync_rx.recv().fuse();
                        pin_mut!(timer, signal);
                        select! {
                            _ = timer => break,
                            result = signal => {
                                match result {
                                    Ok(SyncSignal::Immediate) => {
                                        // Shorten the deadline to 2 s from now, but
                                        // only if that is sooner than the current end.
                                        let shortened =
                                            std::time::Instant::now() + SYNC_DEBOUNCE;
                                        if shortened < debounce_end {
                                            debounce_end = shortened;
                                        }
                                    }
                                    Ok(SyncSignal::LocalChange) => {
                                        // Additional local changes don't extend the wait.
                                    }
                                    Err(_) => return,
                                }
                            }
                        }
                    }
                    // Drain any remaining queued signals.
                    while sync_rx.try_recv().is_ok() {}
                }

                record_poll_at(&weak, cx);
                run_sync_once(&weak, cx).await;
            }
        },
    )
}

/// Compute the poll-timer interval from the current app state.
///
/// This is a pure function of (has_pending, offline, server_rejecting,
/// window_active) exposed as a separate free function so it can be unit-tested.
pub(crate) fn sync_poll_interval(
    has_pending: bool,
    offline: bool,
    server_rejecting: bool,
    window_active: bool,
) -> StdDuration {
    if has_pending && !server_rejecting {
        // Pending edits exist and the server hasn't rejected them (i.e. either
        // we're offline or haven't tried yet): retry aggressively.
        SYNC_PENDING_RETRY
    } else if offline {
        SYNC_POLL_OFFLINE
    } else if window_active {
        SYNC_POLL_FOREGROUND
    } else {
        SYNC_POLL_BACKGROUND
    }
}

fn poll_interval(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) -> StdDuration {
    weak.update(cx, |app, _cx| {
        let has_pending = app.state.has_pending_crdt_edits() || app.sync_pending_hint > 0;
        let offline = app.sync_offline;
        let server_rejecting = app.sync_server_rejecting;
        let window_active = app.window_is_active;
        sync_poll_interval(has_pending, offline, server_rejecting, window_active)
    })
    .unwrap_or(SYNC_POLL_FOREGROUND)
}

fn record_poll_at(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    let now = Utc::now();
    let _ = weak.update(cx, |app, _cx| {
        app.last_sync_poll_at = Some(now);
    });
}

async fn run_sync_once(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    // Refresh the (short-lived) access token before syncing if it's near expiry,
    // persisting the rotated credentials immediately so a rotated refresh token is
    // never lost to a later sync failure. Aborts this tick if the session is gone.
    if ensure_fresh_token(weak, cx).await.is_err() {
        return;
    }

    let snapshot = weak
        .update(cx, |app, _cx| {
            if app.workspace_save_blocked_reason.is_some() {
                return None;
            }
            let account = app.settings.sync_account.clone()?;
            if !account.supports_sync {
                app.sync_run_status = SyncRunStatus::Idle;
                _cx.notify();
                return None;
            }
            app.state.sync_store_from_workspace();
            let pending = app.state.pending_crdt_edits();
            let crdt_states = app.state.crdt_document_states();
            app.sync_run_status = SyncRunStatus::Running {
                pending: pending.len(),
            };
            _cx.notify();
            let notification_schedule = crate::notifications::notification_schedule_snapshot(
                &app.workspace,
                app.settings.notification_defaults,
                Utc::now(),
                0,
            );
            let snapshot = SyncSnapshot {
                workspace: app.workspace.clone(),
                account,
                replica_id: app.settings.replica_id,
                pending,
                crdt_states,
                notification_schedule,
            };
            // Captured after sync_store_from_workspace above, so it only moves
            // again if the user edits while the run is in flight.
            Some((snapshot, app.state.local_edit_watermark()))
        })
        .ok()
        .flatten();

    let Some((snapshot, local_edit_watermark)) = snapshot else {
        return;
    };

    let result = cx
        .background_executor()
        .spawn(async move { sync_snapshot(snapshot) })
        .await;

    match result {
        Ok(result) => {
            let remote_updates_applied = result.remote_updates_applied;
            let pushed = result.pushed.clone();
            let workspace = result.workspace.clone();
            let crdt_states = result.crdt_states.clone();
            let remaining_pending = result.remaining_pending;
            let local_workspace_changed = result.local_workspace_changed;
            let media_downloaded = result.media_downloaded;
            let _ = weak.update(cx, |app, cx| {
                for pushed in pushed {
                    app.state
                        .clear_pushed_crdt_edits(pushed.document, pushed.through_local_sequence);
                }
                app.sync_run_status = SyncRunStatus::Synced {
                    pending: remaining_pending,
                };
                app.sync_pending_hint = remaining_pending;
                app.sync_offline = false;
                app.sync_server_rejecting = false;
                app.last_synced_at = Some(Utc::now());
                if remote_updates_applied > 0 || local_workspace_changed {
                    let scheme_scroll_restore = if app.selection.view == View::Scheme {
                        app.selection
                            .scheme_id
                            .map(|scheme_id| (scheme_id, app.scheme_scroll_handle.offset()))
                    } else {
                        None
                    };
                    let daily_queue_scroll_restore = (app.selection.view == View::DailyQueue)
                        .then(|| app.daily_queue_scroll_handle.offset());
                    // Edits applied while the run was in flight are not in its
                    // result; merge the result into the live documents so they
                    // survive (e.g. an event being drafted on the calendar)
                    // instead of being rolled back until the next round trip.
                    // With no in-flight edits the replace is equivalent and
                    // adopts the run's canonical merged state wholesale.
                    let merged = app.state.has_local_edits_since(local_edit_watermark)
                        && app
                            .state
                            .merge_workspace_from_sync(&workspace, &crdt_states);
                    if !merged {
                        app.state
                            .replace_workspace_from_sync(workspace, crdt_states);
                    }
                    app.scheme_scroll_restore_after_sync = scheme_scroll_restore;
                    app.daily_queue_scroll_restore_after_sync = daily_queue_scroll_restore;
                    app.service_bus.signal_save();
                    app.service_bus.signal_notifications();
                    app.service_bus.signal_timeline();
                }
                if media_downloaded {
                    if let Some((_, editor)) = app.scheme_editor.clone() {
                        editor.update(cx, |_, cx| cx.notify());
                    }
                    for editor in app
                        .daily_queue_editors
                        .values()
                        .cloned()
                        .collect::<Vec<_>>()
                    {
                        editor.update(cx, |_, cx| cx.notify());
                    }
                    app.service_bus.signal_timeline();
                }
                cx.notify();
            });
        }
        Err(err) => {
            eprintln!("sync failed: {err:#}");
            let is_offline = err.downcast_ref::<SyncNetworkUnreachable>().is_some();
            let message = format!("{err:#}");
            // The store queue alone undercounts after a restart, when unpushed
            // edits live only in the persisted sync state — and the poll cadence
            // keys off pending-ness, so count both.
            let disk_pending = load_local_sync_state(&workspace_path())
                .map(|state| state.pending.len())
                .unwrap_or(0);
            let _ = weak.update(cx, |app, cx| {
                let pending_len = app.state.pending_crdt_edits().len().max(disk_pending);
                app.sync_pending_hint = pending_len;
                app.sync_offline = is_offline;
                app.sync_server_rejecting = !is_offline;
                app.sync_run_status = SyncRunStatus::Error {
                    message,
                    pending: pending_len,
                };
                cx.notify();
            });
        }
    }
}

// Returns Err(()) only when the session is gone (refresh token dead) and the
// caller should abort the sync tick; the user has been signed out. Otherwise
// Ok(()), having either refreshed-and-persisted new tokens or left the current
// (still-valid-enough) ones in place.
async fn ensure_fresh_token(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
) -> Result<(), ()> {
    let account = weak
        .update(cx, |app, _cx| app.settings.sync_account.clone())
        .ok()
        .flatten();
    let Some(account) = account else {
        return Ok(());
    };
    // Refresh even when sync isn't entitled: rotation recomputes entitlement
    // server-side and updates the local `supports_sync` cache, so a subscription
    // purchased on the website reaches a signed-in client within one token
    // lifetime instead of never.
    // Only refresh when the access token is at/near expiry.
    if account.expires_at > Utc::now() + chrono::Duration::seconds(ACCESS_REFRESH_SKEW_SECS) {
        return Ok(());
    }
    let Some(refresh_token) = account.refresh_token.clone() else {
        let _ = weak.update(cx, |app, cx| {
            app.settings.sync_account = None;
            app.sync_auth_status = SyncAuthStatus::Error(
                "Your sync session expired. Please sign in again.".to_string(),
            );
            app.sync_run_status = SyncRunStatus::Idle;
            app.save_app_settings();
            cx.notify();
        });
        return Err(());
    };
    let api_base = account.api_base.clone();
    let outcome = cx
        .background_executor()
        .spawn(async move { refresh_sync_backend(&api_base, &refresh_token) })
        .await;
    match outcome {
        Ok(tokens) => {
            let _ = weak.update(cx, |app, cx| {
                if let Some(acct) = app.settings.sync_account.as_mut() {
                    acct.bearer_token = tokens.bearer_token;
                    acct.expires_at = tokens.expires_at;
                    acct.refresh_token = Some(tokens.refresh_token);
                    acct.refresh_expires_at = tokens.refresh_expires_at;
                    acct.supports_sync = tokens.supports_sync;
                    acct.account_status =
                        Some(SyncAccountStatus::from_supports_sync(tokens.supports_sync));
                }
                app.save_app_settings();
                cx.notify();
            });
            Ok(())
        }
        Err(RefreshError::Unauthorized) => {
            // Refresh token revoked/expired/replayed: the session is gone.
            let _ = weak.update(cx, |app, cx| {
                app.settings.sync_account = None;
                app.sync_auth_status = SyncAuthStatus::Error(
                    "Your sync session expired. Please sign in again.".to_string(),
                );
                app.sync_run_status = SyncRunStatus::Idle;
                app.save_app_settings();
                cx.notify();
            });
            Err(())
        }
        Err(RefreshError::Transient(error)) => {
            // Network/parse hiccup: keep the current token and retry next tick.
            eprintln!("sync token refresh deferred: {error:#}");
            Ok(())
        }
    }
}

fn sync_snapshot(snapshot: SyncSnapshot) -> Result<SyncRunResult> {
    let path = workspace_path();
    let mut workspace = workspace_for_background_sync(&path, snapshot.workspace);
    let server_workspace_id = sync_workspace_id(&snapshot.account, workspace.id);
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
    // Restore the long-lived CRDT documents from disk and overlay the UI store's
    // latest states (the `snapshot`), so the sync's CRDT carries this device's stable
    // deterministic identity plus its newest local edits — never rebuilt from plain
    // data. Disk fills documents the in-memory store doesn't hold (e.g. archived /
    // off-screen Daily Queue schemes loaded by `workspace_for_background_sync`).
    let mut crdt_states = load_crdt_state(&path).unwrap_or_default();
    crdt_states.extend(snapshot.crdt_states);
    let mut crdt_docs =
        WorkspaceCrdtDocuments::from_states(&workspace, snapshot.replica_id, &crdt_states)?;
    let mut pushed = Vec::new();

    // One batched pull syncs the whole workspace: the server returns the current
    // merged state of every document whose seq advanced past our cursor (and any
    // document created on another device). Applying it materializes the merged
    // workspace; the engine applies the workspace index before scheme content so
    // newly discovered schemes route correctly.
    let pull = batch_pull_and_apply(
        &client,
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
    let media_downloaded = download_missing_media_assets(&client, &workspace)?;

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
        &client,
        &mut local_state,
        replica_id,
        &snapshot.notification_schedule,
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

    Ok(SyncRunResult {
        workspace,
        crdt_states: merged_crdt_states,
        pushed,
        remote_updates_applied,
        remaining_pending: local_state.pending.len(),
        local_workspace_changed: local_workspace_changed || repaired_workspace_changed,
        media_downloaded,
    })
}

fn queue_repair_crdt_updates(
    local_state: &mut LocalSyncState,
    workspace: &Workspace,
    replica_id: ReplicaId,
    crdt_docs: &mut WorkspaceCrdtDocuments,
) -> Result<()> {
    let outcome = crdt_docs.sync_changes(workspace, &WorkspaceCrdtChangeSet::default().workspace());
    if !outcome.is_ok() {
        return Err(anyhow!("CRDT repair update failed: {:?}", outcome.errors));
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

fn workspace_for_background_sync(path: &std::path::Path, current: Workspace) -> Workspace {
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
    local_state.workspace_id = Some(sync_workspace_id(account, workspace_id));
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

fn workspace_media_assets(workspace: &Workspace) -> Vec<SyncMediaAsset> {
    let mut seen = HashSet::new();
    let mut assets = Vec::new();
    for scheme in workspace.iter_schemes() {
        let Some(meta) = workspace.scheme_sync.get(&scheme.id) else {
            continue;
        };
        for item in &scheme.items {
            for image in item_image_assets(item) {
                let media = SyncMediaAsset {
                    document: meta.id,
                    asset: image.asset,
                    format: image.format,
                };
                if seen.insert(media) {
                    assets.push(media);
                }
            }
        }
    }
    assets
}

fn item_image_assets(item: &Item) -> Vec<ImageInline> {
    let mut images = Vec::new();
    collect_item_image_assets(item, &mut images);
    images
}

fn collect_item_image_assets(item: &Item, images: &mut Vec<ImageInline>) {
    for inline in &item.content {
        match inline {
            Inline::Text { .. } => {}
            Inline::Image(image) => images.push(*image),
            Inline::Table(table) => {
                for cell in table.cells() {
                    for item in &cell.items {
                        collect_item_image_assets(item, images);
                    }
                }
            }
        }
    }
}

fn upload_local_media_assets(
    client: &SyncHttpClient,
    local_state: &mut LocalSyncState,
    workspace: &Workspace,
    remote_latest: &HashMap<DocumentId, u64>,
) -> Result<()> {
    for media in workspace_media_assets(workspace) {
        let path = image_asset_path(media.asset, media.format.extension());
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let byte_length = metadata.len();
        if byte_length == 0 {
            continue;
        }
        if byte_length > MAX_SYNC_MEDIA_BYTES as u64 {
            return Err(anyhow!(
                "image {} is {} bytes, above the {} byte sync limit",
                media.image_name(),
                byte_length,
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        let image_name = media.image_name();
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "image {} is {} bytes, above the {} byte sync limit",
                image_name,
                bytes.len(),
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        let sha256 = media_sha256(&bytes);
        if !local_state.should_upload_media_asset(
            &image_name,
            media.document,
            byte_length,
            &sha256,
            remote_latest,
        ) {
            continue;
        }
        client.upload_media_asset(media, &bytes)?;
        local_state.mark_media_uploaded(image_name, media.document, byte_length, sha256);
    }
    Ok(())
}

fn download_missing_media_assets(client: &SyncHttpClient, workspace: &Workspace) -> Result<bool> {
    let mut downloaded = false;
    for media in workspace_media_assets(workspace) {
        let path = image_asset_path(media.asset, media.format.extension());
        if !media_asset_needs_download(&path)? {
            continue;
        }
        let image_name = media.image_name();
        let Some(bytes) = client.download_media_asset(media)? else {
            eprintln!("sync media missing on backend: {image_name}; skipping download");
            continue;
        };
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "downloaded image {} is {} bytes, above the {} byte sync limit",
                image_name,
                bytes.len(),
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
        downloaded = true;
    }
    Ok(downloaded)
}

fn media_asset_needs_download(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => Ok(false),
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(anyhow!(
            "image asset path {} exists but is not a file",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).with_context(|| format!("stat {}", path.display())),
    }
}

impl SyncTransport for SyncHttpClient {
    fn pull(&self, request: &BatchPullRequest) -> Result<BatchPullResponse> {
        let url = format!("{}/v1/sync/pull", self.api_base);
        self.post_json(&url, request)
    }

    fn push(&self, request: &BatchPushRequest) -> Result<BatchPushResponse> {
        let url = format!("{}/v1/sync/push", self.api_base);
        self.post_json(&url, request)
    }
}

impl SyncHttpClient {
    fn upload_media_asset(&self, media: SyncMediaAsset, bytes: &[u8]) -> Result<()> {
        let url = self.media_url(media);
        self.authorized(ureq::put(&url))
            .set("content-type", media_content_type(media.format))
            .send_bytes(bytes)
            .map_err(sync_http_error)?;
        Ok(())
    }

    fn download_media_asset(&self, media: SyncMediaAsset) -> Result<Option<Vec<u8>>> {
        let url = self.media_url(media);
        let response = match self.authorized(ureq::get(&url)).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(404, response)) => {
                let code = response
                    .into_json::<ErrorResponse>()
                    .map(|error| error.code)
                    .unwrap_or_else(|_| "404".to_string());
                if code == "not_found" {
                    return Ok(None);
                }
                return Err(anyhow!("sync backend rejected request: {code}"));
            }
            Err(error) => return Err(sync_http_error(error)),
        };
        let mut reader = response
            .into_reader()
            .take((MAX_SYNC_MEDIA_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .with_context(|| format!("read media response from {url}"))?;
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "sync backend returned image {} above the {} byte sync limit",
                media.image_name(),
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        Ok(Some(bytes))
    }

    fn media_url(&self, media: SyncMediaAsset) -> String {
        format!(
            "{}/v1/sync/documents/{}/media/{}",
            self.api_base,
            media.document,
            media.image_name()
        )
    }

    fn post_json<T, R>(&self, url: &str, body: &T) -> Result<R>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.authorized(ureq::post(url))
            .send_json(serde_json::to_value(body)?)
            .map_err(sync_http_error)?
            .into_json()
            .with_context(|| format!("parse sync response from {url}"))
    }

    fn authorized(&self, request: ureq::Request) -> ureq::Request {
        // Individual HTTP requests are given 30 s to complete regardless of the
        // current poll cadence.
        const HTTP_TIMEOUT: StdDuration = StdDuration::from_secs(30);
        request
            .timeout(HTTP_TIMEOUT)
            .set("authorization", &format!("Bearer {}", self.bearer_token))
    }
}

fn media_content_type(format: ImageAssetFormat) -> &'static str {
    match format {
        ImageAssetFormat::Png => "image/png",
        ImageAssetFormat::Jpeg => "image/jpeg",
        ImageAssetFormat::Webp => "image/webp",
        ImageAssetFormat::Gif => "image/gif",
        ImageAssetFormat::Svg => "image/svg+xml",
        ImageAssetFormat::Bmp => "image/bmp",
        ImageAssetFormat::Tiff => "image/tiff",
    }
}

fn media_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sync_http_error(error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(status, response) => {
            let code = response
                .into_json::<ErrorResponse>()
                .map(|error| error.code)
                .unwrap_or_else(|_| status.to_string());
            anyhow!("sync backend rejected request: {code}")
        }
        // Transport / connection failures: attach SyncNetworkUnreachable so the
        // scheduler can detect "offline" via downcast_ref.
        error => anyhow::Error::new(SyncNetworkUnreachable)
            .context(format!("sync backend request failed: {error}")),
    }
}

fn normalize_api_base(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!("sync API URL is empty"));
    }
    // The bearer token and all workspace contents travel over this URL. Refuse
    // plaintext HTTP to anything other than a loopback dev server so a misconfig
    // (or tampered settings file) can't silently leak credentials in the clear.
    if !is_secure_api_base(trimmed) {
        return Err(anyhow!("sync API URL must use https:// (got {trimmed})"));
    }
    Ok(trimmed.to_string())
}

fn is_secure_api_base(url: &str) -> bool {
    if let Some(host) = url.strip_prefix("https://") {
        return !host.is_empty();
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest
            .split(['/', ':'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        return matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{
        media_asset_needs_download, normalize_api_base, sync_poll_interval,
        workspace_for_background_sync, SyncNetworkUnreachable, SYNC_PENDING_RETRY,
        SYNC_POLL_BACKGROUND, SYNC_POLL_FOREGROUND, SYNC_POLL_OFFLINE,
    };
    use chrono::{NaiveDate, Utc};
    use knotq_model::{
        daily_queue_scheme_id, DocumentId, OperationId, ReplicaId, Scheme, SyncDocumentKind,
        Workspace, WorkspaceId,
    };
    use knotq_storage_json::{load_workspace_with_options, save_workspace, WorkspaceLoadOptions};
    use knotq_sync::{
        queue_workspace_bootstrap_updates, DocumentSyncCursor, LocalSyncState, PendingCrdtEdit,
        SyncDocumentRef, WorkspaceCrdtDocuments,
    };
    use std::{fs, path::PathBuf};

    #[test]
    fn https_urls_are_accepted_and_trimmed() {
        assert_eq!(
            normalize_api_base("https://sync.example.com/").unwrap(),
            "https://sync.example.com"
        );
    }

    #[test]
    fn loopback_http_is_allowed_for_dev() {
        assert_eq!(
            normalize_api_base("http://localhost:8787").unwrap(),
            "http://localhost:8787"
        );
        assert!(normalize_api_base("http://127.0.0.1:8787").is_ok());
    }

    #[test]
    fn plaintext_http_to_remote_hosts_is_rejected() {
        assert!(normalize_api_base("http://sync.example.com").is_err());
        assert!(normalize_api_base("ftp://sync.example.com").is_err());
        assert!(normalize_api_base("").is_err());
    }

    #[test]
    fn zero_byte_desktop_media_file_is_downloaded_again() {
        let dir = unique_temp_dir("knotq-desktop-media");
        let path = dir.join("asset.png");
        fs::write(&path, []).unwrap();

        assert!(media_asset_needs_download(&path).unwrap());

        fs::write(&path, [1, 2, 3]).unwrap();
        assert!(!media_asset_needs_download(&path).unwrap());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn known_documents_do_not_need_repeated_upserts() {
        let document = DocumentId::new();
        let doc = SyncDocumentRef {
            document,
            kind: SyncDocumentKind::Scheme,
        };
        let mut state = LocalSyncState::default();

        assert!(state.should_upsert_document(doc));

        state.document_cursors.insert(
            document,
            DocumentSyncCursor {
                document,
                kind: SyncDocumentKind::Scheme,
                last_pulled_sequence: 1,
                last_pushed_sequence: 1,
            },
        );

        assert!(!state.should_upsert_document(doc));
    }

    #[test]
    fn background_sync_loads_full_daily_queue_without_losing_memory_edits() {
        let dir = unique_temp_dir("knotq-sync-full-load");
        let path = dir.join("workspace.json");
        let today = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let old_daily_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let mut workspace = Workspace::new();
        let active = Scheme::new("Disk Name", 0);
        let active_id = active.id;
        let old_daily_id = daily_queue_scheme_id(old_daily_date);
        let mut old_daily = Scheme::new("Old Daily", 0);
        old_daily.id = old_daily_id;
        workspace.schemes.insert(active_id, active);
        workspace.schemes.insert(old_daily_id, old_daily);
        workspace.daily_queue.insert(old_daily_date, old_daily_id);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(knotq_model::NodeRef::Scheme(active_id));
        save_workspace(&path, &workspace).unwrap();

        let mut partial = load_workspace_with_options(
            &path,
            WorkspaceLoadOptions::daily_queue_range(today, today),
        )
        .unwrap()
        .unwrap();
        assert!(!partial.schemes.contains_key(&old_daily_id));
        partial.schemes.get_mut(&active_id).unwrap().name = "Memory Name".into();

        let merged = workspace_for_background_sync(&path, partial);

        assert!(merged.schemes.contains_key(&old_daily_id));
        assert_eq!(merged.schemes[&active_id].name, "Memory Name");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bootstrap_snapshot_supersedes_pending_delta_for_new_remote_document() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Unsynced", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let document = workspace.scheme_sync.get(&scheme_id).unwrap().id;
        let replica_id = ReplicaId::new();
        let stale_delta = vec![1, 2, 3];
        let mut state = LocalSyncState {
            workspace_id: Some(workspace.id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.document_cursors.insert(
            document,
            DocumentSyncCursor {
                document,
                kind: SyncDocumentKind::Scheme,
                last_pulled_sequence: 0,
                last_pushed_sequence: 12,
            },
        );
        state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace.id,
            replica_id,
            local_sequence: 1,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1: stale_delta.clone(),
        });

        queue_workspace_bootstrap_updates(
            &mut state,
            &mut WorkspaceCrdtDocuments::try_new(&workspace).unwrap(),
            &workspace,
            replica_id,
            &std::collections::HashMap::new(),
        );

        let pending = state
            .pending
            .iter()
            .filter(|edit| edit.document == document)
            .collect::<Vec<_>>();
        assert_eq!(pending.len(), 1);
        assert_ne!(pending[0].update_v1, stale_delta);
        knotq_sync::validate_crdt_update_sequence(
            SyncDocumentKind::Scheme,
            [pending[0].update_v1.as_slice()],
        )
        .unwrap();
    }

    #[test]
    fn bootstrap_preserves_valid_pending_base_for_new_remote_document() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Unsynced", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let document = workspace.scheme_sync.get(&scheme_id).unwrap().id;
        let valid_base = WorkspaceCrdtDocuments::snapshot_updates(&workspace)
            .updates
            .into_iter()
            .find(|update| update.document == document)
            .unwrap()
            .update_v1;
        let replica_id = ReplicaId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace.id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace.id,
            replica_id,
            local_sequence: 1,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1: valid_base.clone(),
        });

        queue_workspace_bootstrap_updates(
            &mut state,
            &mut WorkspaceCrdtDocuments::try_new(&workspace).unwrap(),
            &workspace,
            replica_id,
            &std::collections::HashMap::new(),
        );

        let pending = state
            .pending
            .iter()
            .filter(|edit| edit.document == document)
            .collect::<Vec<_>>();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].update_v1, valid_base);
    }

    #[test]
    fn bootstrap_drops_orphaned_pending_delta_without_remote_base() {
        // A delta queued for a scheme that has since been deleted (so it is no
        // longer in the workspace) and that the server has no base snapshot for
        // can never be accepted — pushing it trips `crdt_schema_invalid` and wedges
        // the whole push loop. Bootstrap must drop it.
        let mut workspace = Workspace::new();
        workspace.ensure_sync_metadata();
        let replica_id = ReplicaId::new();
        let orphan_document = DocumentId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace.id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace.id,
            replica_id,
            local_sequence: 1,
            created_at: Utc::now(),
            document: orphan_document,
            kind: SyncDocumentKind::Scheme,
            update_v1: vec![9, 9, 9],
        });

        // No remote_latest entry for the orphan document → server has no base.
        queue_workspace_bootstrap_updates(
            &mut state,
            &mut WorkspaceCrdtDocuments::try_new(&workspace).unwrap(),
            &workspace,
            replica_id,
            &std::collections::HashMap::new(),
        );

        assert!(
            !state
                .pending
                .iter()
                .any(|edit| edit.document == orphan_document),
            "orphaned pending delta should be dropped"
        );
    }

    #[test]
    fn bootstrap_reseeds_document_with_stale_cursor_when_server_lacks_base() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Cursor stale", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();
        let document = workspace.scheme_sync.get(&scheme_id).unwrap().id;
        let replica_id = ReplicaId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace.id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.document_cursors.insert(
            document,
            DocumentSyncCursor {
                document,
                kind: SyncDocumentKind::Scheme,
                last_pulled_sequence: 12,
                last_pushed_sequence: 12,
            },
        );

        // The authoritative server head map is empty after a durable-object purge,
        // so the stale local cursor must not suppress a full snapshot bootstrap.
        queue_workspace_bootstrap_updates(
            &mut state,
            &mut WorkspaceCrdtDocuments::try_new(&workspace).unwrap(),
            &workspace,
            replica_id,
            &std::collections::HashMap::new(),
        );

        let pending = state
            .pending
            .iter()
            .filter(|edit| edit.document == document)
            .collect::<Vec<_>>();
        assert_eq!(pending.len(), 1);
        knotq_sync::validate_crdt_update_sequence(
            SyncDocumentKind::Scheme,
            [pending[0].update_v1.as_slice()],
        )
        .unwrap();
    }

    #[test]
    fn recovery_heal_clears_pull_cursors_exactly_once() {
        let document = DocumentId::new();
        let workspace_document = DocumentId::new();
        let cursor = DocumentSyncCursor {
            document,
            kind: SyncDocumentKind::Scheme,
            last_pulled_sequence: 9,
            last_pushed_sequence: 4,
        };
        let mut state = LocalSyncState::default();
        state.document_cursors.insert(document, cursor.clone());
        state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: WorkspaceId::new(),
            replica_id: ReplicaId::new(),
            local_sequence: 1,
            created_at: Utc::now(),
            document: workspace_document,
            kind: SyncDocumentKind::PersonalWorkspace,
            update_v1: vec![1],
        });
        state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: WorkspaceId::new(),
            replica_id: ReplicaId::new(),
            local_sequence: 2,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1: vec![2],
        });
        state.mark_media_uploaded("asset.png".to_string(), document, 4, "hash".to_string());

        // A pre-recovery file (version 0) heals once: cursors are dropped so the
        // next sync re-pulls and re-merges from zero, and stale workspace-index
        // deltas are not allowed to re-push the corrupt index.
        assert!(state.heal_for_recovery_version());
        assert!(state.document_cursors.is_empty());
        assert!(state.media_cursors.is_empty());
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].kind, SyncDocumentKind::Scheme);
        assert_eq!(
            state.recovery_version,
            knotq_sync::SYNC_STATE_RECOVERY_VERSION
        );

        // Idempotent afterward: an already-healed file is left untouched.
        state.document_cursors.insert(document, cursor);
        assert!(!state.heal_for_recovery_version());
        assert_eq!(state.document_cursors.len(), 1);
    }

    #[test]
    fn bootstrap_keeps_orphan_delta_when_server_has_a_base() {
        // If the server does have a base for the (now-removed) document, its deltas
        // can still be applied, so they must be preserved rather than dropped.
        let mut workspace = Workspace::new();
        workspace.ensure_sync_metadata();
        let replica_id = ReplicaId::new();
        let document = DocumentId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace.id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace.id,
            replica_id,
            local_sequence: 1,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1: vec![4, 5, 6],
        });

        let mut remote_latest = std::collections::HashMap::new();
        remote_latest.insert(document, 7u64);
        queue_workspace_bootstrap_updates(
            &mut state,
            &mut WorkspaceCrdtDocuments::try_new(&workspace).unwrap(),
            &workspace,
            replica_id,
            &remote_latest,
        );

        assert!(
            state.pending.iter().any(|edit| edit.document == document),
            "delta with a server base must be preserved"
        );
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    // ── sync_poll_interval tests ───────────────────────────────────────────

    #[test]
    fn poll_interval_pending_no_server_rejection_is_pending_retry() {
        assert_eq!(
            sync_poll_interval(true, false, false, true),
            SYNC_PENDING_RETRY
        );
        // Offline with pending edits also uses PENDING_RETRY (cheap local failure).
        assert_eq!(
            sync_poll_interval(true, true, false, true),
            SYNC_PENDING_RETRY
        );
    }

    #[test]
    fn poll_interval_pending_server_rejecting_falls_back_to_foreground() {
        // Server rejection: don't hammer the backend; use foreground cadence.
        assert_eq!(
            sync_poll_interval(true, false, true, true),
            SYNC_POLL_FOREGROUND
        );
    }

    #[test]
    fn poll_interval_pending_server_rejecting_background_uses_background() {
        assert_eq!(
            sync_poll_interval(true, false, true, false),
            SYNC_POLL_BACKGROUND
        );
    }

    #[test]
    fn poll_interval_offline_no_pending_is_offline() {
        assert_eq!(
            sync_poll_interval(false, true, false, true),
            SYNC_POLL_OFFLINE
        );
        assert_eq!(
            sync_poll_interval(false, true, false, false),
            SYNC_POLL_OFFLINE
        );
    }

    #[test]
    fn poll_interval_foreground_active() {
        assert_eq!(
            sync_poll_interval(false, false, false, true),
            SYNC_POLL_FOREGROUND
        );
    }

    #[test]
    fn poll_interval_background_inactive() {
        assert_eq!(
            sync_poll_interval(false, false, false, false),
            SYNC_POLL_BACKGROUND
        );
    }

    // ── SyncNetworkUnreachable downcast test ───────────────────────────────

    #[test]
    fn sync_network_unreachable_downcasts_through_anyhow_context() {
        let err = anyhow::Error::new(SyncNetworkUnreachable)
            .context("sync backend request failed: some io error");
        assert!(
            err.downcast_ref::<SyncNetworkUnreachable>().is_some(),
            "downcast_ref should find SyncNetworkUnreachable through context chain"
        );
    }

    #[test]
    fn non_network_error_does_not_downcast_to_unreachable() {
        let err = anyhow::anyhow!("sync backend rejected request: forbidden");
        assert!(err.downcast_ref::<SyncNetworkUnreachable>().is_none());
    }
}
