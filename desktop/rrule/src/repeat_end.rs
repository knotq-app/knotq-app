use chrono::{DateTime, Utc};
use knotq_model::RepeatEnd;

pub(crate) fn repeat_end_allows(end: &RepeatEnd, index: usize, anchor: DateTime<Utc>) -> bool {
    match end {
        RepeatEnd::Never => true,
        RepeatEnd::Count(count) => index < *count,
        RepeatEnd::Until(until) => anchor <= *until,
    }
}

pub fn validate_repeat_end(end: &RepeatEnd) -> bool {
    match end {
        RepeatEnd::Never => true,
        RepeatEnd::Count(count) => *count > 0,
        RepeatEnd::Until(_) => true,
    }
}
