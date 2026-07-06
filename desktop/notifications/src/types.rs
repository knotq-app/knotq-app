use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use knotq_model::{
    ItemId, ItemKind, OccurrenceId, SchemeId, DEFAULT_ASSIGNMENT_NOTIFICATION_OFFSET_SECS,
    DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
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
    /// Stable identity for an occurrence's notification.
    ///
    /// Deliberately excludes `fire_at`: the key must stay constant when a
    /// reschedule ("remind me later") shifts the fire time, so that the
    /// already-delivered notification on every device shares an id with the
    /// rescheduled one and can be cleared. `fire_at` changes are still detected
    /// for reschedule purposes via the manifest fingerprint, which hashes it
    /// separately (see `schedule::request_fingerprint`).
    ///
    /// Daily-queue schemes use a constant scheme fragment instead of their
    /// per-day id: "roll over from yesterday" moves an item (same id) from one
    /// day's scheme to the next, and the key must survive that hop so the
    /// carried item keeps its pending schedule, delivered banner, and snooze
    /// state on every device. Item ids are unique, so the constant fragment
    /// cannot collide across days.
    pub(crate) fn make_key(
        scheme: SchemeId,
        scheme_is_daily: bool,
        item: ItemId,
        occurrence: &OccurrenceId,
        kind: NotificationKind,
    ) -> String {
        let scheme_fragment = if scheme_is_daily {
            "daily".to_string()
        } else {
            scheme.0.to_string()
        };
        format!(
            "{}|{}|{}|{}",
            scheme_fragment,
            item.0,
            occurrence_key_fragment(occurrence),
            match kind {
                NotificationKind::Reminder => "r",
                NotificationKind::Event => "e",
                NotificationKind::Assignment => "a",
            },
        )
    }
}

fn occurrence_key_fragment(occurrence: &OccurrenceId) -> String {
    match occurrence {
        OccurrenceId::Single => "single".to_string(),
        OccurrenceId::Recurring { original_start } => original_start.as_utc_lossy().to_rfc3339(),
    }
}
