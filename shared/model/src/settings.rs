use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ReplicaId;

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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncAccountSettings {
    pub api_base: String,
    pub user_id: String,
    pub email: String,
    #[serde(default = "default_true")]
    pub supports_sync: bool,
    pub bearer_token: String,
    pub expires_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
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

fn default_event_notification_offset_secs() -> i64 {
    0
}

fn default_assignment_notification_offset_secs() -> i64 {
    2 * 60 * 60
}
