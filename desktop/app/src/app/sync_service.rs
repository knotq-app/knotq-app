use std::collections::HashMap;
use std::time::Duration as StdDuration;

use knotq_model::{
    DocumentId, ImageAssetFormat, NotificationDefaults, ReplicaId, SyncAccountSettings, Workspace,
};
use knotq_sync::{NotificationScheduleSnapshot, PendingCrdtEdit, PushedDocument};
use std::fmt;

mod http;
mod media;
mod snapshot;
mod tasks;
mod ws_lifecycle;
#[cfg(feature = "ws-sync")]
mod ws_socket;
mod ws_transport;

pub(crate) use tasks::spawn_sync_task;

#[cfg(test)]
use http::normalize_api_base;
#[cfg(test)]
use media::media_asset_needs_download;
#[cfg(test)]
use snapshot::workspace_for_background_sync;
#[cfg(test)]
use tasks::sync_poll_interval;

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
// When the WebSocket transport is connected, pushes ride a persistent socket with
// no per-edit connection cost, so the long HTTP-era debounces above collapse to
// near-real-time windows that coalesce a burst of keystrokes (send) or a burst of
// server `changed` nudges (receive) into a single sync. They apply only while
// connected; the HTTP fallback keeps the longer debounces so an offline device
// doesn't hammer the backend with a connection per edit.
//
// The local-change window is the time we wait after the *first* keystroke of a
// burst before a sync run. It only delays how fast *our* edits reach peers —
// inbound peer edits still arrive instantly via the `changed` nudge (which uses the
// 150 ms Immediate window), and unpushed edits are already saved locally, so a
// larger window costs nothing but a little outbound convergence latency. Keeping it
// at 2 s (vs the old 300 ms) collapses a multi-second typing burst into a single
// sync run instead of ~one every 300 ms, which is the main lever on how often the
// per-run snapshot work (workspace clone + CRDT encode) runs while typing.
const SYNC_LOCAL_CHANGE_DEBOUNCE_WS: StdDuration = StdDuration::from_secs(2);
const SYNC_DEBOUNCE_WS: StdDuration = StdDuration::from_millis(150);
const SYNC_PENDING_RETRY: StdDuration = StdDuration::from_secs(30);
const SYNC_POLL_FOREGROUND: StdDuration = StdDuration::from_secs(120);
const SYNC_POLL_BACKGROUND: StdDuration = StdDuration::from_secs(30 * 60);
const SYNC_POLL_OFFLINE: StdDuration = StdDuration::from_secs(20 * 60);
// When the WebSocket is connected, server `changed` nudges (and an on-(re)connect
// catch-up) drive syncs in real time, so a *foreground* device does NOT poll the
// network at all. The timer below is only a short LOCAL re-check tick: when it
// fires in foreground-WS mode the loop skips the network sync entirely (see
// `foreground_ws_idle`), and only re-evaluates connectivity so it can resume
// polling promptly if the socket has dropped. Backgrounded, the WS-connected case
// keeps a slow real heartbeat instead (`SYNC_POLL_WS_CONNECTED`).
const SYNC_POLL_WS_IDLE_RECHECK: StdDuration = StdDuration::from_secs(60);
const SYNC_POLL_WS_CONNECTED: StdDuration = StdDuration::from_secs(30 * 60);
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

/// Marker error attached when the backend rejects the bearer token (HTTP 401 /
/// `unauthorized`). The scheduler reacts by force-refreshing the access token and
/// retrying the run once — the local expiry check alone can't catch a token the
/// server rejects early (revocation, key rotation, clock skew).
#[derive(Debug)]
struct SyncUnauthorized;

impl fmt::Display for SyncUnauthorized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sync backend rejected the access token")
    }
}

impl std::error::Error for SyncUnauthorized {}

/// Marker error attached when the backend rejects a sync request because this
/// build's wire protocol is below the server's configured floor (HTTP 426 /
/// `client_protocol_outdated`). Unlike `SyncPushRejected`, this must NOT trigger
/// the engine's reseed self-heal — reseeding and re-pushing would just be
/// rejected the same way until the app is updated. There is currently no
/// automatic recovery: the scheduler leaves this as a plain error so the
/// existing sync-error banner surfaces it, and it clears on the next successful
/// sync after the user updates.
#[derive(Debug)]
struct SyncProtocolOutdated;

impl fmt::Display for SyncProtocolOutdated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("this version of KnotQ is too old to sync — please update the app")
    }
}

impl std::error::Error for SyncProtocolOutdated {}

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
    /// Lead-time defaults for the notification schedule. The schedule itself
    /// (recurrence expansion + per-occurrence hashing — the heaviest snapshot step)
    /// is computed on the background sync thread from `workspace`, not on main.
    notification_defaults: NotificationDefaults,
    /// The schedule computed by a previous run, reused as-is when nothing that could
    /// change it has happened since (same generation + defaults + day — decided on
    /// main in `run_sync_once`). `None` forces a recompute on the background thread.
    reuse_schedule: Option<NotificationScheduleSnapshot>,
    /// The live WebSocket sync client, if connected. The run prefers it over HTTP
    /// (see `ws_transport::FallbackTransport`); `None` falls back to HTTP only.
    ws_sync: Option<std::sync::Arc<knotq_sync::ws::WsClient>>,
    /// Whether this run may propose a history squash after finishing fully
    /// synced. Throttled by the scheduler so a server-declined proposal (e.g.
    /// `squash_too_soon`) is not rebuilt and re-sent on every poll.
    allow_squash: bool,
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
    /// The schedule used by this run (reused or freshly computed), handed back so the
    /// caller can cache it for the next run's reuse check.
    notification_schedule: NotificationScheduleSnapshot,
    /// True when this run built and sent a squash proposal (accepted or not),
    /// so the scheduler can arm its attempt throttle.
    squash_attempted: bool,
}

/// A notification schedule cached on `KnotQApp` between sync runs, with the inputs
/// it was computed from. The next run reuses `snapshot` only when the live
/// generation, defaults, and current day all still match — otherwise it recomputes.
#[derive(Clone)]
pub(crate) struct CachedNotificationSchedule {
    pub(crate) generation: u64,
    pub(crate) defaults: NotificationDefaults,
    pub(crate) snapshot: NotificationScheduleSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SyncMediaAsset {
    document: DocumentId,
    asset: uuid::Uuid,
    format: ImageAssetFormat,
}

struct SyncHttpClient {
    api_base: String,
    bearer_token: String,
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
                epoch: 0,
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
                epoch: 0,
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
            touched_items: Vec::new(),
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
            touched_items: Vec::new(),
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
            touched_items: Vec::new(),
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
                epoch: 0,
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
            epoch: 0,
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
            touched_items: Vec::new(),
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
            touched_items: Vec::new(),
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
            touched_items: Vec::new(),
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
