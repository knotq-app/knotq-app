use std::collections::{HashMap, HashSet, VecDeque};

mod crdt;
mod engine;
mod fractional;

use chrono::{DateTime, Utc};
use knotq_model::{
    DocumentId, OperationId, ReplicaId, ShareId, SyncDocumentKind, Workspace, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crdt::{
    validate_crdt_update_sequence, WorkspaceCrdtApplyOutcome, WorkspaceCrdtChangeSet,
    WorkspaceCrdtDocuments, WorkspaceCrdtSyncOutcome, YrsSchemeDocument,
};
pub use engine::{
    batch_pull_and_apply, batch_push_pending, PullOutcome, PushedDocument, SyncTransport,
    PUSH_MAX_DOCUMENTS_PER_REQUEST, PUSH_MAX_UPDATES_PER_DOCUMENT,
};

/// Serde codec that represents CRDT update bytes as a base64 string rather than
/// a JSON array of integers. The array form inflates each byte to up to four
/// JSON characters (`"255,"`); base64 keeps the wire/at-rest payload ~1.33x the
/// raw size instead of ~4-8x.
mod base64_bytes {
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
mod base64_bytes_vec {
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
pub const MAX_SYNC_MEDIA_BYTES: usize = 3 * 1024 * 1024;

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
pub const SYNC_STATE_RECOVERY_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub Uuid);

impl UserId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for UserId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_period_end: Option<DateTime<Utc>>,
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

impl AccessRole {
    pub fn can_read(self) -> bool {
        matches!(self, Self::Owner | Self::Writer | Self::Reader)
    }

    pub fn can_write(self) -> bool {
        matches!(self, Self::Owner | Self::Writer)
    }
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

pub fn sync_documents(workspace: &Workspace) -> Vec<SyncDocumentRef> {
    let mut docs = vec![SyncDocumentRef {
        document: workspace.sync.id,
        kind: SyncDocumentKind::PersonalWorkspace,
    }];
    docs.extend(scheme_documents(workspace));
    docs
}

pub fn scheme_documents(workspace: &Workspace) -> Vec<SyncDocumentRef> {
    workspace
        .scheme_sync
        .values()
        .filter(|meta| meta.kind == SyncDocumentKind::Scheme)
        .map(|meta| SyncDocumentRef {
            document: meta.id,
            kind: SyncDocumentKind::Scheme,
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingCrdtEdit {
    pub operation_id: OperationId,
    pub workspace_id: WorkspaceId,
    pub replica_id: ReplicaId,
    pub local_sequence: u64,
    pub created_at: DateTime<Utc>,
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    #[serde(with = "base64_bytes")]
    pub update_v1: Vec<u8>,
}

impl PendingCrdtEdit {
    pub fn as_update(&self) -> CrdtDocumentUpdate {
        CrdtDocumentUpdate {
            document: self.document,
            kind: self.kind,
            update_v1: self.update_v1.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentSyncCursor {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    #[serde(default)]
    pub last_pulled_sequence: u64,
    #[serde(default)]
    pub last_pushed_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MediaSyncCursor {
    pub image_name: String,
    pub document: DocumentId,
    pub byte_length: u64,
    #[serde(default)]
    pub sha256: String,
    pub uploaded_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalSyncState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replica_id: Option<ReplicaId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub document_cursors: HashMap<DocumentId, DocumentSyncCursor>,
    #[serde(default)]
    pub media_cursors: HashMap<String, MediaSyncCursor>,
    #[serde(default)]
    pub pending: VecDeque<PendingCrdtEdit>,
    /// Last applied recovery generation (see [`SYNC_STATE_RECOVERY_VERSION`]).
    /// Absent in older files, so it defaults to 0 and triggers the heal.
    #[serde(default)]
    pub recovery_version: u32,
}

impl LocalSyncState {
    pub fn is_configured(&self) -> bool {
        self.workspace_id.is_some()
            && self.replica_id.is_some()
            && self
                .server_url
                .as_deref()
                .is_some_and(|url| !url.is_empty())
            && self
                .bearer_token
                .as_deref()
                .is_some_and(|token| !token.is_empty())
    }

    pub fn replace_pending(&mut self, pending: impl IntoIterator<Item = PendingCrdtEdit>) {
        self.pending = pending.into_iter().collect();
    }

    /// Apply any pending one-time recovery for the current
    /// [`SYNC_STATE_RECOVERY_VERSION`]. Clears pull cursors so the next sync
    /// re-pulls every document from sequence zero and re-merges (idempotent in
    /// Yjs), repairing an on-disk workspace that diverged from advanced cursors.
    /// Workspace-index pending edits are dropped during recovery because older
    /// clients could queue deltas from a partial/corrupt workspace index. Scheme
    /// content edits are left intact. Returns `true` if a heal was applied.
    pub fn heal_for_recovery_version(&mut self) -> bool {
        if self.recovery_version >= SYNC_STATE_RECOVERY_VERSION {
            return false;
        }
        self.document_cursors.clear();
        self.pending
            .retain(|edit| edit.kind != SyncDocumentKind::PersonalWorkspace);
        self.recovery_version = SYNC_STATE_RECOVERY_VERSION;
        true
    }

    pub fn push_pending(&mut self, edit: PendingCrdtEdit) {
        self.pending.push_back(edit);
    }

    pub fn pending_for_document(&self, document: DocumentId, limit: usize) -> Vec<PendingCrdtEdit> {
        self.pending
            .iter()
            .filter(|edit| edit.document == document)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn pending_document_sequence_is_valid(
        &self,
        document: DocumentId,
        kind: SyncDocumentKind,
    ) -> bool {
        let updates = self
            .pending
            .iter()
            .filter(|edit| edit.document == document)
            .map(|edit| edit.update_v1.as_slice())
            .collect::<Vec<_>>();
        !updates.is_empty() && validate_crdt_update_sequence(kind, updates).is_ok()
    }

    pub fn should_upsert_document(&self, doc: SyncDocumentRef) -> bool {
        !self.document_cursors.contains_key(&doc.document)
    }

    pub fn next_push_request(
        &self,
        document: DocumentId,
        limit: usize,
    ) -> Option<PushUpdatesRequest> {
        let replica_id = self.replica_id?;
        let updates = self
            .pending_for_document(document, limit)
            .into_iter()
            .map(|edit| edit.as_update())
            .collect::<Vec<_>>();
        if updates.is_empty() {
            return None;
        }
        Some(PushUpdatesRequest {
            replica_id,
            updates,
            notification_schedule_changed: false,
            notification_schedule: None,
        })
    }

    pub fn mark_pushed(&mut self, document: DocumentId, through_local_sequence: u64) -> usize {
        let before = self.pending.len();
        let mut kind = None;
        self.pending.retain(|edit| {
            if edit.document == document && edit.local_sequence <= through_local_sequence {
                kind = Some(edit.kind);
                false
            } else {
                true
            }
        });
        if let Some(kind) = kind {
            let cursor = self
                .document_cursors
                .entry(document)
                .or_insert(DocumentSyncCursor {
                    document,
                    kind,
                    last_pulled_sequence: 0,
                    last_pushed_sequence: 0,
                });
            cursor.last_pushed_sequence = cursor.last_pushed_sequence.max(through_local_sequence);
        }
        before - self.pending.len()
    }

    pub fn mark_pulled(
        &mut self,
        document: DocumentId,
        kind: SyncDocumentKind,
        latest_sequence: u64,
    ) {
        let cursor = self
            .document_cursors
            .entry(document)
            .or_insert(DocumentSyncCursor {
                document,
                kind,
                last_pulled_sequence: 0,
                last_pushed_sequence: 0,
            });
        cursor.kind = kind;
        cursor.last_pulled_sequence = cursor.last_pulled_sequence.max(latest_sequence);
    }

    pub fn media_upload_is_current(
        &self,
        image_name: &str,
        document: DocumentId,
        byte_length: u64,
        sha256: &str,
    ) -> bool {
        self.media_cursors.get(image_name).is_some_and(|cursor| {
            cursor.document == document
                && cursor.byte_length == byte_length
                && cursor.sha256 == sha256
        })
    }

    pub fn mark_media_uploaded(
        &mut self,
        image_name: String,
        document: DocumentId,
        byte_length: u64,
        sha256: String,
    ) {
        self.media_cursors.insert(
            image_name.clone(),
            MediaSyncCursor {
                image_name,
                document,
                byte_length,
                sha256,
                uploaded_at: Utc::now(),
            },
        );
    }
}

pub fn queue_workspace_bootstrap_updates(
    sync_state: &mut LocalSyncState,
    workspace: &Workspace,
    replica_id: ReplicaId,
    remote_latest: &HashMap<DocumentId, u64>,
) {
    let mut next_sequence = sync_state
        .pending
        .iter()
        .map(|edit| edit.local_sequence)
        .max()
        .unwrap_or(0)
        + 1;
    let mut bootstrapped: HashSet<DocumentId> = HashSet::new();
    for update in WorkspaceCrdtDocuments::snapshot_updates(workspace).updates {
        if remote_latest.get(&update.document).copied().unwrap_or(0) != 0 {
            continue;
        }
        if sync_state.pending_document_sequence_is_valid(update.document, update.kind) {
            bootstrapped.insert(update.document);
            continue;
        }
        // If local deltas were queued before the first successful upload, they
        // cannot be applied on the server without a base document. Trust the
        // server's zero sequence over any stale local cursor, then push the
        // current full snapshot first.
        sync_state
            .pending
            .retain(|pending| pending.document != update.document);
        bootstrapped.insert(update.document);
        sync_state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace.id,
            replica_id,
            local_sequence: next_sequence,
            created_at: Utc::now(),
            document: update.document,
            kind: update.kind,
            update_v1: update.update_v1,
        });
        next_sequence += 1;
    }

    // Drop queued deltas that the server can never accept: a document it has no
    // base snapshot for (remote sequence 0) that we also did not just re-seed with
    // a full snapshot above. These orphans appear when a scheme is deleted or its
    // sync-document id is reassigned while edits are still queued. A lone delta
    // reconstructs a document with no `schema` field, which the backend rejects as
    // `crdt_schema_invalid`, wedging the push loop behind the bad edit.
    sync_state.pending.retain(|edit| {
        bootstrapped.contains(&edit.document)
            || remote_latest.get(&edit.document).copied().unwrap_or(0) != 0
    });
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
}
