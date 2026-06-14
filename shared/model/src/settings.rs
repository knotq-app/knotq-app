use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ReplicaId, SchemeId};

/// The screen the user last had open, persisted so the app reopens where it left
/// off. Settings is intentionally not a variant — it's a transient page, so the
/// last *content* view stays saved while it's open.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedView {
    Union,
    DailyQueue,
    Scheme,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CalendarViewMode {
    #[default]
    Week,
    Month,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarWeekRange {
    #[default]
    NextSevenDays,
    CalendarWeek,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    System,
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeFormat {
    #[default]
    TwelveHour,
    TwentyFourHour,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindowSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindowPosition {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub replica_id: ReplicaId,
    #[serde(default)]
    pub calendar_view: CalendarViewMode,
    #[serde(default)]
    pub calendar_week_range: CalendarWeekRange,
    #[serde(default)]
    pub theme_mode: ThemeMode,
    #[serde(default)]
    pub time_format: TimeFormat,
    #[serde(default)]
    pub notification_defaults: NotificationDefaults,
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scheduled_notification_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_size: Option<SavedWindowSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_position: Option<SavedWindowPosition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub google_accounts: Vec<GoogleOAuthAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_account: Option<SyncAccountSettings>,
    #[serde(default)]
    pub onboarding_completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_view: Option<SavedView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scheme_id: Option<SchemeId>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            replica_id: ReplicaId::default(),
            calendar_view: CalendarViewMode::default(),
            calendar_week_range: CalendarWeekRange::default(),
            theme_mode: ThemeMode::default(),
            time_format: TimeFormat::default(),
            notification_defaults: NotificationDefaults::default(),
            auto_update: default_true(),
            scheduled_notification_ids: Vec::new(),
            window_size: None,
            window_position: None,
            google_accounts: Vec::new(),
            sync_account: None,
            onboarding_completed: false,
            last_view: None,
            last_scheme_id: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncAccountSettings {
    pub api_base: String,
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub email: String,
    #[serde(default = "default_true")]
    pub supports_sync: bool,
    /// Short-lived access token; `expires_at` is its expiry.
    pub bearer_token: String,
    pub expires_at: DateTime<Utc>,
    /// Long-lived refresh credential and its (sliding) expiry. Optional so older
    /// persisted settings (a single long-lived bearer token) still deserialize; a
    /// missing refresh token simply forces a one-time re-login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_status: Option<SyncAccountStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncAccountStatus {
    #[serde(default = "default_account_level")]
    pub level: String,
    #[serde(default)]
    pub subscribed: bool,
    #[serde(default = "default_true")]
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

impl SyncAccountStatus {
    pub fn from_supports_sync(supports_sync: bool) -> Self {
        Self {
            level: if supports_sync { "sync" } else { "free" }.to_string(),
            subscribed: supports_sync,
            supports_sync,
            subscription_status: Some(
                if supports_sync { "active" } else { "inactive" }.to_string(),
            ),
            subscription_provider: None,
            current_period_end: None,
            checked_at: None,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_account_level() -> String {
    "free".to_string()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoogleOAuthAccount {
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub scope: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NotificationDefaults {
    #[serde(default = "default_event_notification_offset_secs")]
    pub event_offset_secs: i64,
    #[serde(default = "default_assignment_notification_offset_secs")]
    pub assignment_offset_secs: i64,
}

impl Default for NotificationDefaults {
    fn default() -> Self {
        Self {
            event_offset_secs: default_event_notification_offset_secs(),
            assignment_offset_secs: default_assignment_notification_offset_secs(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NotificationLeadTimes {
    pub event: i64,
    pub assignment: i64,
    pub reminder: i64,
}

pub const DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS: i64 = 10 * 60;
pub const DEFAULT_ASSIGNMENT_NOTIFICATION_OFFSET_SECS: i64 = 2 * 60 * 60;

fn default_event_notification_offset_secs() -> i64 {
    DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS
}

fn default_assignment_notification_offset_secs() -> i64 {
    DEFAULT_ASSIGNMENT_NOTIFICATION_OFFSET_SECS
}
