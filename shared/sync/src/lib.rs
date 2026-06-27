use std::collections::HashMap;

mod access;
mod crdt;
mod documents;
mod engine;
mod fractional;
mod local_state;
mod persisted_state;
mod user_id;
pub mod ws;

use chrono::{DateTime, Utc};
use knotq_model::{DocumentId, ReplicaId, ShareId, SyncDocumentKind, WorkspaceId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crdt::{
    validate_crdt_update_sequence, DocumentApplyError, WorkspaceApplyError,
    WorkspaceCrdtApplyOutcome, WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments,
    WorkspaceCrdtSyncOutcome, YrsSchemeDocument,
};
pub use engine::{
    batch_pull_and_apply, batch_push_pending, PullOutcome, PushedDocument, SkippedDocument,
    SyncPushRejected, SyncTransport, PUSH_MAX_DOCUMENTS_PER_REQUEST, PUSH_MAX_UPDATES_PER_DOCUMENT,
};
pub use documents::{scheme_documents, sync_documents};
pub use local_state::{
    queue_account_switch_reseed, queue_workspace_bootstrap_updates, DocumentSyncCursor,
    LocalSyncState, MediaSyncCursor, PendingCrdtEdit,
};

/// Serde codec that represents CRDT update bytes as a base64 string rather than
/// a JSON array of integers. The array form inflates each byte to up to four
/// JSON characters (`"255,"`); base64 keeps the wire/at-rest payload ~1.33x the
/// raw size instead of ~4-8x.
pub(crate) mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        STANDARD
            .decode(encoded.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

/// Like [`base64_bytes`] but for a list of update payloads, so a batched push can
/// carry `["<b64>", "<b64>", …]` on the wire instead of nested integer arrays.
pub(crate) mod base64_bytes_vec {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(items: &[Vec<u8>], serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(items.len()))?;
        for item in items {
            seq.serialize_element(&STANDARD.encode(item))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Vec<u8>>, D::Error> {
        Vec::<String>::deserialize(deserializer)?
            .into_iter()
            .map(|encoded| {
                STANDARD
                    .decode(encoded.as_bytes())
                    .map_err(serde::de::Error::custom)
            })
            .collect()
    }
}

pub const SYNC_API_VERSION: &str = "2026-06-08-crdt-sync-batched";
pub const LOCAL_SYNC_STATE_FILE: &str = "sync-state.json";
/// On-disk file holding each CRDT document's persisted `state_v1`. The CRDT
/// documents are long-lived: drivers restore them from this file (with a stable,
/// deterministic clientID) instead of rebuilding from plain data, so the Yjs
/// identity survives restarts and the desktop UI↔background thread split.
pub const LOCAL_CRDT_STATE_FILE: &str = "sync-crdt-state.json";
pub const MAX_SYNC_MEDIA_BYTES: usize = 3 * 1024 * 1024;

/// Serializable container for persisted per-document CRDT `state_v1` bytes
/// (base64-encoded on the wire/at rest). Round-trips the map produced by
/// [`WorkspaceCrdtDocuments::document_states`] and consumed by
/// [`WorkspaceCrdtDocuments::from_states`].
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PersistedCrdtState {
    #[serde(default)]
    pub documents: Vec<PersistedDocumentState>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PersistedDocumentState {
    pub document: DocumentId,
    #[serde(with = "base64_bytes")]
    pub state_v1: Vec<u8>,
}

/// One-time local-state recovery generation. Bump this when a fixed sync bug could
/// have left persisted `document_cursors` pointing past data that never made it
/// into the on-disk workspace. On load, a client whose stored generation is lower
/// clears its pull cursors once, forcing an idempotent full re-pull/re-merge that
/// repairs the divergence. Generation 1 recovers from the push-failure desync that
/// advanced cursors without persisting the merged workspace (dropping other
/// devices' schemes and re-activating archived ones). Generation 2 reruns the
/// recovery after fixing CRDT materialization of archived and Daily Queue schemes
/// and drops stale workspace-index deltas so they cannot re-push the bad shape.
/// Generation 3 reruns the one-time pull-cursor reset for the merged-state batched
/// sync protocol (2026-06-08): the per-document append-log endpoints were replaced
/// by `/v1/sync/pull` + `/v1/sync/push`, and document `seq` is now a per-document
/// version counter rather than a global append sequence. Clearing cursors makes the
/// first sync re-pull every document's merged state and re-converge idempotently.
/// Generation 4 canonicalizes account workspace root folders and Daily Queue
/// scheme identities so independently-created devices do not merge duplicate
/// roots or visible Daily Queue documents into the sidebar tree.
/// Generation 5 accompanies the persisted-CRDT / deterministic-clientID fix
/// (2026-06-08): clients no longer rebuild the Yjs documents from plain data with a
/// throwaway clientID on every sync, which had let renames lose to stale
/// re-encodings and dropped cross-device schemes. Clearing cursors once forces a
/// full re-pull so each device adopts the server's canonical document identities and
/// re-converges; the persisted CRDT-state file then keeps that identity stable.
/// Generation 6 clears media upload cursors once so clients re-upload local image
/// bytes after the media-sync recovery. CRDT metadata can converge while raw image
/// objects are absent, especially after a durable-object/object-store reset.
pub const SYNC_STATE_RECOVERY_VERSION: u32 = 6;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub Uuid);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
}

impl ErrorResponse {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DevLoginRequest {
    pub email: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountSignupRequest {
    pub email: String,
    pub password: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountLoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountResponse {
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub email: String,
    #[serde(default = "default_supports_sync")]
    pub supports_sync: bool,
}

/// Response from `GET /v1/auth/account/status`.
///
/// `level` is intentionally a string rather than an enum so the backend can add
/// tiers such as "pro", "team", or "beta" without requiring an app update.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountStatusResponse {
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub email: String,
    #[serde(default = "default_account_level")]
    pub level: String,
    #[serde(default)]
    pub subscribed: bool,
    #[serde(default = "default_supports_sync")]
    pub supports_sync: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_status: Option<String>,
    /// Normalized lifecycle independent of provider: `active` (entitled, renews),
    /// `cancelled` (entitled until `current_period_end` but won't renew), or
    /// `inactive`. Optional so responses from older backends still deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_period_end: Option<DateTime<Utc>>,
    /// Whether the account email has been confirmed. Subscribing is gated on this.
    /// The current backend always sends it; the `default` (false) only applies to a
    /// response that omits it, which fails closed (treat as unverified) rather than
    /// silently granting — the backend remains the authoritative gate either way.
    #[serde(default)]
    pub email_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthSession {
    pub session_id: String,
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub email: String,
    #[serde(default = "default_supports_sync")]
    pub supports_sync: bool,
    /// Short-lived access token; `expires_at` is its expiry.
    pub bearer_token: String,
    pub expires_at: DateTime<Utc>,
    /// Long-lived, single-use, rotated-on-refresh credential presented to
    /// `POST /v1/auth/refresh`; `refresh_expires_at` is its (sliding) expiry.
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub refresh_expires_at: Option<DateTime<Utc>>,
}

pub fn default_supports_sync() -> bool {
    true
}

pub fn default_account_level() -> String {
    "free".to_string()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RegisterDeviceRequest {
    pub replica_id: ReplicaId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub platform: DevicePlatform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_channel: Option<PushChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_environment: Option<PushEnvironment>,
    #[serde(default)]
    pub notification_permission: NotificationPermissionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_scheduler_supported: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RegisterDeviceResponse {
    pub workspace_id: WorkspaceId,
    pub replica_id: ReplicaId,
    pub notification_schedule_revision: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    Ios,
    Android,
    Macos,
    Windows,
    Linux,
    Web,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushChannel {
    Apns,
    Fcm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushEnvironment {
    Sandbox,
    Production,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPermissionState {
    Granted,
    Denied,
    Provisional,
    Ephemeral,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpsertDocumentRequest {
    pub kind: SyncDocumentKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentResponse {
    pub workspace_id: WorkspaceId,
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    pub owner: UserId,
    pub role: AccessRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRole {
    Owner,
    Writer,
    Reader,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShareDocumentRequest {
    pub grantee: UserId,
    #[serde(default = "default_share_role")]
    pub role: AccessRole,
}

fn default_share_role() -> AccessRole {
    AccessRole::Writer
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShareDocumentResponse {
    pub workspace_id: WorkspaceId,
    pub document: DocumentId,
    pub share_id: ShareId,
    pub grantee: UserId,
    pub role: AccessRole,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrdtDocumentUpdate {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    #[serde(with = "base64_bytes")]
    pub update_v1: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyncDocumentRef {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PushUpdatesRequest {
    pub replica_id: ReplicaId,
    pub updates: Vec<CrdtDocumentUpdate>,
    #[serde(default)]
    pub notification_schedule_changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule: Option<NotificationScheduleSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NotificationScheduleSnapshot {
    pub sequence: u64,
    pub hash: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub occurrence_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PushUpdatesResponse {
    pub accepted: usize,
    pub latest_sequence: u64,
    pub notification_schedule_revision: u64,
    pub background_pushes_enqueued: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PullUpdatesResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<StoredCrdtSnapshot>,
    #[serde(default)]
    pub forced_snapshot: bool,
    pub updates: Vec<StoredCrdtUpdate>,
    pub latest_sequence: u64,
    pub notification_schedule_revision: u64,
    /// Set by the server when more updates remain beyond this page; the client
    /// should keep pulling (advancing `after`) until this is false.
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredCrdtSnapshot {
    pub workspace_id: WorkspaceId,
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    pub sequence: u64,
    pub compacted_at: DateTime<Utc>,
    #[serde(with = "base64_bytes")]
    pub update_v1: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredCrdtUpdate {
    pub workspace_id: WorkspaceId,
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    pub replica_id: ReplicaId,
    pub sequence: u64,
    pub received_at: DateTime<Utc>,
    #[serde(with = "base64_bytes")]
    pub update_v1: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Batched sync protocol (2026-06-08). One round-trip syncs the whole workspace:
// `pull` returns the current merged state of every changed document, `push` merges
// a batch of documents' updates server-side. This replaces the per-document
// pull/push fan-out (one HTTP request per document) and the append-only update log.
// ---------------------------------------------------------------------------

/// `POST /v1/sync/pull`. Cursors map each known document to the last `seq` this
/// replica applied; the server returns merged state for any document past that
/// (a missing/zero cursor also discovers documents created on other devices).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BatchPullRequest {
    pub replica_id: ReplicaId,
    #[serde(default)]
    pub cursors: HashMap<DocumentId, u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PulledCrdtDocument {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    pub seq: u64,
    #[serde(with = "base64_bytes")]
    pub state_v1: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BatchPullResponse {
    #[serde(default)]
    pub documents: Vec<PulledCrdtDocument>,
    /// Authoritative server-side document heads after this pull. This lets clients
    /// distinguish "no changed documents" from "the server has no copy of a
    /// document named by a stale local cursor" after a workspace purge/reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_documents: Option<HashMap<DocumentId, u64>>,
    #[serde(default)]
    pub notification_schedule_revision: u64,
    /// More changed documents remain beyond the per-response cap; the client should
    /// pull again with advanced cursors until this is false.
    #[serde(default)]
    pub has_more: bool,
}

/// One document's batch of CRDT updates within a [`BatchPushRequest`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PushDocumentUpdates {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    #[serde(with = "base64_bytes_vec")]
    pub updates: Vec<Vec<u8>>,
}

/// `POST /v1/sync/push`. Every dirty document in one request; each document's
/// updates are merged into its stored state (one server row write per document).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BatchPushRequest {
    pub replica_id: ReplicaId,
    pub documents: Vec<PushDocumentUpdates>,
    #[serde(default)]
    pub notification_schedule_changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule: Option<NotificationScheduleSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PushedCrdtDocument {
    pub document: DocumentId,
    pub seq: u64,
    pub accepted: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BatchPushResponse {
    #[serde(default)]
    pub documents: Vec<PushedCrdtDocument>,
    #[serde(default)]
    pub notification_schedule_revision: u64,
    #[serde(default)]
    pub background_pushes_enqueued: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MarkNotificationScheduleChangedRequest {
    pub replica_id: ReplicaId,
    #[serde(default)]
    pub reason: NotificationScheduleChangeReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scheduled_occurrence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_permission: Option<NotificationPermissionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_scheduler_supported: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationScheduleChangeReason {
    #[default]
    ReminderChanged,
    SyncUpdate,
    PermissionChanged,
    ManualResync,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MarkNotificationScheduleChangedResponse {
    pub workspace_id: WorkspaceId,
    pub replica_id: ReplicaId,
    pub notification_schedule_revision: u64,
    pub background_pushes_enqueued: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AckNotificationScheduleRequest {
    pub replica_id: ReplicaId,
    pub notification_schedule_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scheduled_occurrence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_permission: Option<NotificationPermissionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_scheduler_supported: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AckNotificationScheduleResponse {
    pub workspace_id: WorkspaceId,
    pub replica_id: ReplicaId,
    pub accepted_revision: u64,
    pub server_schedule_revision: u64,
    pub up_to_date: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NotificationScheduleStatusResponse {
    pub workspace_id: WorkspaceId,
    pub notification_schedule_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<DateTime<Utc>>,
    pub devices: Vec<DeviceNotificationScheduleStatus>,
    pub pending_background_pushes: Vec<BackgroundPushIntent>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListDevicesResponse {
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub devices: Vec<DeviceSyncStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceSyncStatus {
    pub replica_id: ReplicaId,
    pub registered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_permission: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_scheduler_supported: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_window_start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_window_end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_occurrence_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceNotificationScheduleStatus {
    pub replica_id: ReplicaId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub platform: DevicePlatform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_channel: Option<PushChannel>,
    pub has_push_token: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_environment: Option<PushEnvironment>,
    pub notification_permission: NotificationPermissionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_scheduler_supported: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_window_start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_window_end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_schedule_updated_at: Option<DateTime<Utc>>,
    pub acknowledged_schedule_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ack_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_until: Option<DateTime<Utc>>,
    pub scheduled_occurrence_count: usize,
    pub schedule_status: NotificationScheduleDeviceState,
    pub pending_background_push_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_background_push_enqueued_at: Option<DateTime<Utc>>,
    pub registered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationScheduleDeviceState {
    Fresh,
    Stale,
    PermissionDenied,
    LocalSchedulingUnavailable,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackgroundPushIntent {
    pub id: u64,
    pub workspace_id: WorkspaceId,
    pub target_replica_id: ReplicaId,
    pub notification_schedule_revision: u64,
    pub kind: BackgroundPushKind,
    pub reason: NotificationScheduleChangeReason,
    pub enqueued_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundPushKind {
    NotificationScheduleChanged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::OperationId;

    #[test]
    fn local_sync_state_builds_document_push_requests() {
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let document = DocumentId::new();
        let mut state = LocalSyncState {
            workspace_id: Some(workspace_id),
            replica_id: Some(replica_id),
            ..LocalSyncState::default()
        };
        state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id,
            replica_id,
            local_sequence: 1,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1: vec![1, 2, 3],
        });

        let request = state.next_push_request(document, 10).unwrap();

        assert_eq!(request.replica_id, replica_id);
        assert_eq!(request.updates.len(), 1);
        assert_eq!(request.updates[0].document, document);
    }

    #[test]
    fn crdt_update_bytes_serialize_as_base64_string() {
        let update = CrdtDocumentUpdate {
            document: DocumentId::new(),
            kind: SyncDocumentKind::Scheme,
            update_v1: vec![0, 1, 2, 255],
        };
        let json = serde_json::to_value(&update).unwrap();
        // base64 of [0,1,2,255] is "AAEC/w==" — a string, not a JSON array.
        assert_eq!(json["update_v1"], serde_json::json!("AAEC/w=="));
        let round_tripped: CrdtDocumentUpdate = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped.update_v1, vec![0, 1, 2, 255]);
    }

    #[test]
    fn batched_sync_requests_do_not_serialize_auth_credentials() {
        let replica_id = ReplicaId::new();
        let document = DocumentId::new();
        let pull = BatchPullRequest {
            replica_id,
            cursors: HashMap::from([(document, 7)]),
        };
        let push = BatchPushRequest {
            replica_id,
            documents: vec![PushDocumentUpdates {
                document,
                kind: SyncDocumentKind::Scheme,
                updates: vec![vec![1, 2, 3]],
            }],
            notification_schedule_changed: true,
            notification_schedule: Some(NotificationScheduleSnapshot {
                sequence: 3,
                hash: "schedule-hash".to_string(),
                window_start: Utc::now(),
                window_end: Utc::now(),
                occurrence_count: 1,
            }),
        };

        let pull_json = serde_json::to_string(&pull).unwrap();
        let push_json = serde_json::to_string(&push).unwrap();
        for json in [pull_json, push_json] {
            assert!(!json.contains("refresh_token"));
            assert!(!json.contains("bearer_token"));
            assert!(!json.contains("SYNC_REFRESH_SECRET"));
            assert!(!json.contains("GOOGLE_REFRESH_SECRET"));
        }
    }

    #[test]
    fn local_sync_state_does_not_persist_legacy_bearer_token() {
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let raw = serde_json::json!({
            "workspace_id": workspace_id,
            "replica_id": replica_id,
            "server_url": "https://api.example.com",
            "bearer_token": "LEGACY_BEARER_TOKEN"
        });

        let state: LocalSyncState = serde_json::from_value(raw).unwrap();
        assert!(state.is_configured());
        let serialized = serde_json::to_string(&state).unwrap();
        assert!(!serialized.contains("bearer_token"));
        assert!(!serialized.contains("LEGACY_BEARER_TOKEN"));
    }

    #[test]
    fn mark_pushed_removes_only_acknowledged_document_edits() {
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let left = DocumentId::new();
        let right = DocumentId::new();
        let mut state = LocalSyncState::default();
        for (document, sequence) in [(left, 1), (right, 2), (left, 3)] {
            state.push_pending(PendingCrdtEdit {
                operation_id: OperationId::new(),
                workspace_id,
                replica_id,
                local_sequence: sequence,
                created_at: Utc::now(),
                document,
                kind: SyncDocumentKind::Scheme,
                update_v1: vec![sequence as u8],
            });
        }

        assert_eq!(state.mark_pushed(left, 1), 1);

        assert_eq!(state.pending.len(), 2);
        assert!(state.pending.iter().any(|edit| edit.document == right));
        assert!(state
            .pending
            .iter()
            .any(|edit| edit.document == left && edit.local_sequence == 3));
    }

    #[test]
    fn media_upload_retries_when_server_lacks_document_base() {
        let document = DocumentId::new();
        let image_name = "asset.png".to_string();
        let bytes = b"image bytes";
        let sha256 = "same-hash".to_string();
        let mut state = LocalSyncState::default();
        state.mark_media_uploaded(
            image_name.clone(),
            document,
            bytes.len() as u64,
            sha256.clone(),
        );

        assert!(state.should_upload_media_asset(
            &image_name,
            document,
            bytes.len() as u64,
            &sha256,
            &HashMap::new(),
        ));

        let mut remote_latest = HashMap::new();
        remote_latest.insert(document, 4);
        assert!(!state.should_upload_media_asset(
            &image_name,
            document,
            bytes.len() as u64,
            &sha256,
            &remote_latest,
        ));
        assert!(state.should_upload_media_asset(
            &image_name,
            document,
            bytes.len() as u64,
            "changed-hash",
            &remote_latest,
        ));
    }

    #[test]
    fn legacy_media_cursor_without_sha_reuploads_asset() {
        let document = DocumentId::new();
        let image_name = "legacy.png";
        let raw = serde_json::json!({
            "media_cursors": {
                image_name: {
                    "image_name": image_name,
                    "document": document,
                    "byte_length": 11,
                    "uploaded_at": "2026-06-19T00:00:00Z"
                }
            }
        });
        let state: LocalSyncState = serde_json::from_value(raw).unwrap();
        let remote_latest = HashMap::from([(document, 3)]);

        assert!(state.should_upload_media_asset(
            image_name,
            document,
            11,
            "current-sha256",
            &remote_latest,
        ));
    }
}
