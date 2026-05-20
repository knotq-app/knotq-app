use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::OccurrenceId;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceState {
    #[serde(default, skip_serializing_if = "OccurrenceId::is_single")]
    pub occurrence: OccurrenceId,
    #[serde(flatten)]
    pub state: ItemState,
}

impl Default for OccurrenceState {
    fn default() -> Self {
        Self {
            occurrence: OccurrenceId::Single,
            state: ItemState::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ItemState {
    /// 0 = open, -1 = done. Positive values are reserved for future "in progress" states.
    #[serde(default, skip_serializing_if = "is_zero_i8")]
    pub progress: i8,
    /// Optional per-occurrence override on the lead-time offset (seconds before the trigger date).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_offset_secs: Option<i64>,
}

impl ItemState {
    pub fn is_done(&self) -> bool {
        self.progress < 0
    }

    pub fn is_default(&self) -> bool {
        self.progress == 0 && self.notification_offset_secs.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Occurrence {
    pub id: OccurrenceId,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub available: Option<DateTime<Utc>>,
    pub kind: ItemKind,
    pub occurrence_index: usize,
    pub state: ItemState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ItemKind {
    Reminder,
    Assignment,
    Event,
    Procedure,
}

fn is_zero_i8(value: &i8) -> bool {
    *value == 0
}
