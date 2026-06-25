use chrono::{DateTime, Duration, Utc};
use knotq_date_util::{upcoming_range, week_range, DateRange};
use knotq_model::ItemKind;
use knotq_rrule::{DefaultExpander, OccurrenceExpander};

use crate::calendar::OccurrenceWithContext;
use crate::IndexedWorkspace;

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
        let mut events = self.range(upcoming_range(from));
        events.retain(|event| occurrence_anchor(event) >= Some(from));
        events.truncate(limit);
        events
    }

    pub fn overdue(&self, as_of: DateTime<Utc>) -> Vec<OccurrenceWithContext> {
        self.overdue_retaining(as_of, |_| false)
    }

    /// Overdue assignments/reminders, plus any just-completed occurrence the
    /// caller asks to retain — so checking one off keeps it on the panel (faded)
    /// in place rather than dropping out, mirroring the desktop upcoming panel.
    pub fn overdue_retaining(
        &self,
        as_of: DateTime<Utc>,
        is_retained: impl Fn(&OccurrenceWithContext) -> bool,
    ) -> Vec<OccurrenceWithContext> {
        // A recurring item surfaces at most its few most-recent overdue occurrences
        // so a long-missed daily/weekly habit doesn't flood the panel — but still
        // shows something rather than nothing.
        const MAX_OVERDUE_PER_RECURRING: usize = 5;
        // How far back to look for a recurring item's overdue occurrences. Generous
        // enough to find recent ones at any realistic frequency (daily..monthly)
        // while keeping the per-item expansion bounded.
        const RECURRING_OVERDUE_WINDOW_DAYS: i64 = 180;
        // Non-recurring overdue assignments/reminders surface regardless of age (no
        // lookback window), mirroring the desktop upcoming panel — a single item per
        // entry, so the unbounded range never explodes.
        let range = DateRange {
            start: DateTime::<Utc>::UNIX_EPOCH,
            end: as_of,
        };
        let mut out = Vec::new();
        for context in &self.indexed.calendar.items {
            let Some(scheme) = self.indexed.workspace.scheme(context.scheme_id) else {
                continue;
            };
            let Some(item) = scheme.item(context.item_id) else {
                continue;
            };
            if !matches!(item.kind(), ItemKind::Assignment | ItemKind::Reminder) {
                continue;
            }
            let mut push = |occurrence| {
                out.push(OccurrenceWithContext {
                    occurrence,
                    scheme_id: context.scheme_id,
                    item_id: context.item_id,
                    color_index: context.color_index,
                    scheme_name: context.scheme_name.clone(),
                });
            };
            if item.repeats.is_some() {
                // Recurring: surface only the most-recent overdue occurrences within a
                // bounded recent window — so a long-missed habit of any frequency shows
                // the last few rather than nothing, without materializing its history.
                let window = DateRange {
                    start: as_of - Duration::days(RECURRING_OVERDUE_WINDOW_DAYS),
                    end: as_of,
                };
                let mut occurrences = self.expander.expand(item, window);
                occurrences.retain(|occurrence| occurrence.start.or(occurrence.end) < Some(as_of));
                occurrences.sort_by_key(|occurrence| occurrence.start.or(occurrence.end));
                for occurrence in occurrences.into_iter().rev().take(MAX_OVERDUE_PER_RECURRING) {
                    push(occurrence);
                }
            } else {
                for occurrence in self.expander.expand(item, range) {
                    push(occurrence);
                }
            }
        }
        out.retain(|event| {
            let anchor = match event.occurrence.kind {
                ItemKind::Assignment => event.occurrence.end,
                ItemKind::Reminder => event.occurrence.start,
                _ => None,
            };
            anchor.is_some_and(|anchor| anchor < as_of)
                && (!event.occurrence.state.is_done() || is_retained(event))
        });
        out.sort_by_key(occurrence_sort_key);
        out
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
