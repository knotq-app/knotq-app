use chrono::{DateTime, Duration, Utc};
use knotq_date_util::{week_range, DateRange};
use knotq_model::ItemKind;
use knotq_rrule::{DefaultExpander, OccurrenceExpander};

use crate::calendar::OccurrenceWithContext;
use crate::IndexedWorkspace;

const OVERDUE_LOOKBACK_DAYS: i64 = 7;

pub struct CalendarQuery<'a> {
    indexed: &'a IndexedWorkspace,
    expander: &'a dyn OccurrenceExpander,
}

impl<'a> CalendarQuery<'a> {
    pub fn new(indexed: &'a IndexedWorkspace) -> Self {
        Self {
            indexed,
            expander: &DefaultExpander,
        }
    }

    pub fn with_expander(
        indexed: &'a IndexedWorkspace,
        expander: &'a dyn OccurrenceExpander,
    ) -> Self {
        Self { indexed, expander }
    }

    pub fn range(&self, range: DateRange) -> Vec<OccurrenceWithContext> {
        let mut out = Vec::new();
        for context in &self.indexed.calendar.items {
            let Some(scheme) = self.indexed.workspace.scheme(context.scheme_id) else {
                continue;
            };
            let Some(item) = scheme.item(context.item_id) else {
                continue;
            };
            for occurrence in self.expander.expand(item, range) {
                out.push(OccurrenceWithContext {
                    occurrence,
                    scheme_id: context.scheme_id,
                    item_id: context.item_id,
                    color_index: context.color_index,
                    scheme_name: context.scheme_name.clone(),
                });
            }
        }
        out.sort_by_key(occurrence_sort_key);
        out
    }

    pub fn week(&self, offset: i32, today: chrono::NaiveDate) -> Vec<OccurrenceWithContext> {
        self.range(week_range(offset, today))
    }

    pub fn upcoming(&self, from: DateTime<Utc>, limit: usize) -> Vec<OccurrenceWithContext> {
        let mut events = self.range(DateRange {
            start: from,
            end: from + Duration::days(365),
        });
        events.retain(|event| occurrence_anchor(event) >= Some(from));
        events.truncate(limit);
        events
    }

    pub fn overdue(&self, as_of: DateTime<Utc>) -> Vec<OccurrenceWithContext> {
        let mut events = self.range(DateRange {
            start: as_of - Duration::days(OVERDUE_LOOKBACK_DAYS),
            end: as_of,
        });
        events.retain(|event| {
            event.occurrence.kind == ItemKind::Assignment
                && event.occurrence.end.is_some_and(|end| end < as_of)
                && !event.occurrence.state.is_done()
        });
        events
    }
}

fn occurrence_anchor(event: &OccurrenceWithContext) -> Option<DateTime<Utc>> {
    event
        .occurrence
        .start
        .or(event.occurrence.end)
        .or(event.occurrence.available)
}

fn occurrence_sort_key(event: &OccurrenceWithContext) -> DateTime<Utc> {
    occurrence_anchor(event).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}
