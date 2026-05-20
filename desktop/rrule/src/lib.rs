mod complex;
mod expand;
pub mod ical;
pub mod overrides;
mod repeat_end;
mod scope;
mod simple;

use chrono::{DateTime, Duration, Utc};
use knotq_date_util::DateRange;
use knotq_model::{Item, Occurrence};

pub use expand::*;
pub use repeat_end::validate_repeat_end;
pub use scope::*;

pub trait OccurrenceExpander: Send + Sync {
    fn expand(&self, item: &Item, range: DateRange) -> Vec<Occurrence>;
    fn next_after(&self, item: &Item, after: DateTime<Utc>) -> Option<Occurrence>;
    fn prev_before(&self, item: &Item, before: DateTime<Utc>) -> Option<Occurrence>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultExpander;

impl OccurrenceExpander for DefaultExpander {
    fn expand(&self, item: &Item, range: DateRange) -> Vec<Occurrence> {
        item.occurrences(range.start, range.end)
    }

    fn next_after(&self, item: &Item, after: DateTime<Utc>) -> Option<Occurrence> {
        item.occurrences(after, after + Duration::days(365 * 5))
            .into_iter()
            .filter(|occ| occurrence_anchor(occ) > Some(after))
            .min_by_key(occurrence_anchor)
    }

    fn prev_before(&self, item: &Item, before: DateTime<Utc>) -> Option<Occurrence> {
        item.occurrences(before - Duration::days(365 * 5), before)
            .into_iter()
            .filter(|occ| occurrence_anchor(occ) < Some(before))
            .max_by_key(occurrence_anchor)
    }
}

pub fn expand_item(item: &Item, range: DateRange) -> Vec<Occurrence> {
    item.occurrences(range.start, range.end)
}

fn occurrence_anchor(occurrence: &Occurrence) -> Option<DateTime<Utc>> {
    occurrence.start.or(occurrence.end).or(occurrence.available)
}
