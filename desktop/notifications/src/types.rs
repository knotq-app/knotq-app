use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use knotq_model::{
    ItemId, ItemKind, OccurrenceId, SchemeId, DEFAULT_ASSIGNMENT_NOTIFICATION_OFFSET_SECS,
    DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Reminder,
    Event,
    Assignment,
}

impl NotificationKind {
    pub(crate) fn from_item(kind: ItemKind) -> Option<Self> {
        match kind {
            ItemKind::Reminder => Some(Self::Reminder),
            ItemKind::Event => Some(Self::Event),
            ItemKind::Assignment => Some(Self::Assignment),
            ItemKind::Procedure => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotificationLeadTimes {
    pub reminder_offset_secs: i64,
    pub event_offset_secs: i64,
    pub assignment_offset_secs: i64,
}

impl Default for NotificationLeadTimes {
    fn default() -> Self {
        Self {
            reminder_offset_secs: 0,
            event_offset_secs: DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS,
            assignment_offset_secs: DEFAULT_ASSIGNMENT_NOTIFICATION_OFFSET_SECS,
        }
    }
}

/// Lead-time before the trigger date.
pub fn lead_offset_for_kind(kind: NotificationKind, lead_times: NotificationLeadTimes) -> Duration {
    match kind {
        NotificationKind::Reminder => Duration::seconds(lead_times.reminder_offset_secs),
        NotificationKind::Event => Duration::seconds(lead_times.event_offset_secs),
        NotificationKind::Assignment => Duration::seconds(lead_times.assignment_offset_secs),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduledNotification {
    pub key: String,
    pub fire_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub title: String,
    pub body: String,
    pub kind: NotificationKind,
    pub trigger_at: DateTime<Utc>,
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub occurrence: OccurrenceId,
}

impl ScheduledNotification {
    pub(crate) fn make_key(
        scheme: SchemeId,
        item: ItemId,
        occurrence: &OccurrenceId,
        kind: NotificationKind,
        fire_at: DateTime<Utc>,
    ) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            scheme.0,
            item.0,
            occurrence_key_fragment(occurrence),
            match kind {
                NotificationKind::Reminder => "r",
                NotificationKind::Event => "e",
                NotificationKind::Assignment => "a",
            },
            fire_at.to_rfc3339()
        )
    }
}

fn occurrence_key_fragment(occurrence: &OccurrenceId) -> String {
    match occurrence {
        OccurrenceId::Single => "single".to_string(),
        OccurrenceId::Recurring { original_start } => original_start.as_utc_lossy().to_rfc3339(),
    }
}
