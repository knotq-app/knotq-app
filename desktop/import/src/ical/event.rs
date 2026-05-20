use chrono::{DateTime, Utc};
use knotq_model::CalendarRecurrence;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedEvent {
    pub uid: String,
    pub summary: String,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub recurrence: Option<CalendarRecurrence>,
}
