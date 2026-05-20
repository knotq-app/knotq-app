use chrono::NaiveDate;

use crate::{
    cal_index::{daily_queue_calendar_index_matches_range, SchemeCalendarIndex},
    schema::DailyQueueIndexEntry,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceLoadOptions {
    daily_queue_start: Option<NaiveDate>,
    daily_queue_end: Option<NaiveDate>,
    calendar_query_start: Option<NaiveDate>,
    calendar_query_end: Option<NaiveDate>,
}

impl WorkspaceLoadOptions {
    pub fn all() -> Self {
        Self {
            daily_queue_start: None,
            daily_queue_end: None,
            calendar_query_start: None,
            calendar_query_end: None,
        }
    }

    pub fn daily_queue_range(start: NaiveDate, end: NaiveDate) -> Self {
        let first = start.min(end);
        let last = start.max(end);
        Self {
            daily_queue_start: Some(first),
            daily_queue_end: Some(last),
            calendar_query_start: Some(first),
            calendar_query_end: Some(last),
        }
    }

    fn should_load_daily_queue_date(self, date: NaiveDate) -> bool {
        match (self.daily_queue_start, self.daily_queue_end) {
            (Some(start), Some(end)) => (start..=end).contains(&date),
            _ => true,
        }
    }

    pub(crate) fn should_load_daily_queue_entry(self, entry: &DailyQueueIndexEntry) -> bool {
        self.should_load_daily_queue_date(entry.date)
            || self.should_load_daily_queue_for_calendar_index(&entry.scheme.calendar_index)
    }

    fn should_load_daily_queue_for_calendar_index(self, index: &SchemeCalendarIndex) -> bool {
        daily_queue_calendar_index_matches_range(
            index,
            self.calendar_query_start,
            self.calendar_query_end,
        )
    }
}

impl Default for WorkspaceLoadOptions {
    fn default() -> Self {
        Self::all()
    }
}
