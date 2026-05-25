use chrono::{DateTime, Utc};
use knotq_model::{DocumentId, ReplicaId, ShareId, SyncDocumentKind, WorkspaceId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SYNC_API_VERSION: &str = "2026-05-25-notification-schedule";

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
pub struct AuthSession {
    pub user_id: UserId,
    pub bearer_token: String,
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
    pub update_v1: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PushUpdatesRequest {
    pub replica_id: ReplicaId,
    pub updates: Vec<CrdtDocumentUpdate>,
    #[serde(default)]
    pub notification_schedule_changed: bool,
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
    pub updates: Vec<StoredCrdtUpdate>,
    pub latest_sequence: u64,
    pub notification_schedule_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredCrdtUpdate {
    pub workspace_id: WorkspaceId,
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    pub replica_id: ReplicaId,
    pub sequence: u64,
    pub received_at: DateTime<Utc>,
    pub update_v1: Vec<u8>,
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
