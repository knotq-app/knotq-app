use std::cell::RefCell;
use std::collections::HashMap;

use chrono::DateTime;
use knotq_model::{Item, ItemMarker, Occurrence, OccurrenceState, Recurrence};

use super::*;

/// Cache of expanded recurrence occurrences, keyed by item. Re-expanding every
/// recurring item's RRULE on every calendar repaint is the dominant calendar
/// render cost, yet most repaints (hover, selection, scroll, unrelated state)
/// don't change any item.
///
/// Correctness is *local*: each entry stores the exact inputs `Item::occurrences`
/// reads, and is reused only when those inputs and the query range still match.
/// A stale entry is therefore impossible — if anything that affects the
/// occurrence set changes (dates, recurrence, per-occurrence completion, marker),
/// the entry misses and re-expands. This needs no global "workspace changed"
/// signal, so daily-queue loads, calendar imports, and day rollovers can never
/// leave it stale. Only recurring items are cached; non-recurring expansion is a
/// cheap range check.
#[derive(Default)]
pub(crate) struct CalendarOccurrenceCache {
    entries: RefCell<HashMap<ItemId, CachedOccurrences>>,
}

struct CachedOccurrences {
    inputs: OccurrenceInputs,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    occurrences: Vec<Occurrence>,
}

/// Exactly the subset of an `Item` that `Item::occurrences` depends on.
struct OccurrenceInputs {
    marker: ItemMarker,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    available: Option<DateTime<Utc>>,
    repeats: Option<Recurrence>,
    state: Vec<OccurrenceState>,
}

impl OccurrenceInputs {
    fn capture(item: &Item) -> Self {
        Self {
            marker: item.marker,
            start: item.start,
            end: item.end,
            available: item.available,
            repeats: item.repeats.clone(),
            state: item.state.clone(),
        }
    }

    fn matches(&self, item: &Item) -> bool {
        self.marker == item.marker
            && self.start == item.start
            && self.end == item.end
            && self.available == item.available
            && self.repeats == item.repeats
            && self.state == item.state
    }
}

fn occurrence_task(
    occ: &Occurrence,
    scheme_id: SchemeId,
    color_index: u8,
    item: &Item,
    is_daily: bool,
    is_read_only: bool,
) -> CalendarTask {
    CalendarTask {
        scheme_id,
        item_id: item.id,
        occurrence: occ.id.clone(),
        occurrence_index: occ.occurrence_index,
        color_index,
        is_daily,
        is_read_only,
        text: item.text(),
        start: occ.start.map(|d| d.with_timezone(&Local)),
        end: occ.end.map(|d| d.with_timezone(&Local)),
        kind: occ.kind,
        is_done: occ.state.is_done(),
    }
}

impl KnotQApp {
    pub(super) fn collect_calendar_tasks(
        &self,
        start_utc: chrono::DateTime<Utc>,
        end_utc: chrono::DateTime<Utc>,
    ) -> Vec<CalendarTask> {
        // Carry forward only the entries reused this pass; the rest (vanished
        // items, changed inputs, stale ranges) drop, keeping the cache bounded to
        // the recurring items currently visible.
        let mut prev = self.calendar_occurrence_cache.entries.borrow_mut();
        let mut next: HashMap<ItemId, CachedOccurrences> = HashMap::new();
        let mut all_tasks = Vec::new();
        for scheme in self.workspace.iter_schemes() {
            let is_daily = self.workspace.is_daily_queue_scheme(scheme.id);
            let is_read_only = scheme.is_read_only();
            let color_index = scheme.color_index;
            for item in &scheme.items {
                if item.repeats.is_none() {
                    // Non-recurring: a single-occurrence range check, cheaper than
                    // a cache lookup — expand directly.
                    for occ in item.occurrences(start_utc, end_utc) {
                        all_tasks.push(occurrence_task(
                            &occ,
                            scheme.id,
                            color_index,
                            item,
                            is_daily,
                            is_read_only,
                        ));
                    }
                    continue;
                }

                let entry = match prev.remove(&item.id) {
                    Some(entry)
                        if entry.from == start_utc
                            && entry.to == end_utc
                            && entry.inputs.matches(item) =>
                    {
                        entry
                    }
                    _ => CachedOccurrences {
                        inputs: OccurrenceInputs::capture(item),
                        from: start_utc,
                        to: end_utc,
                        occurrences: item.occurrences(start_utc, end_utc),
                    },
                };
                for occ in &entry.occurrences {
                    all_tasks.push(occurrence_task(
                        occ,
                        scheme.id,
                        color_index,
                        item,
                        is_daily,
                        is_read_only,
                    ));
                }
                next.insert(item.id, entry);
            }
        }
        *prev = next;
        all_tasks
    }
}
