use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use knotq_model::{Item, ItemKind, OccurrenceId, OccurrenceOverrideStatus, SchemeId, Workspace};
use knotq_rrule::ItemOccurrenceExt;

const COMPLETION_LOOKBACK_DAYS: i64 = 7;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CalendarOccurrenceKey {
    pub scheme_id: SchemeId,
    pub item_id: knotq_model::ItemId,
    pub occurrence: OccurrenceId,
}

#[derive(Clone, Debug, Default)]
pub struct RetainedCompletedItems {
    keys: HashSet<CalendarOccurrenceKey>,
}

impl RetainedCompletedItems {
    pub fn contains(&self, key: &CalendarOccurrenceKey) -> bool {
        self.keys.contains(key)
    }

    pub fn insert(&mut self, key: CalendarOccurrenceKey) {
        self.keys.insert(key);
    }

    pub fn remove(&mut self, key: &CalendarOccurrenceKey) {
        self.keys.remove(key);
    }

    pub fn as_set(&self) -> &HashSet<CalendarOccurrenceKey> {
        &self.keys
    }
}

pub fn complete_past_events(state: &mut crate::AppState, now: DateTime<Utc>) -> usize {
    let changed = mark_past_events_done(&mut state.workspace, now);
    if changed > 0 {
        let all_ids: Vec<_> = state.workspace.schemes.keys().copied().collect();
        for id in all_ids {
            state.dirty_schemes.insert(id);
        }
        state.index_dirty = true;
    }
    changed
}

pub fn sync_retained_completed_calendar_items(
    state: &mut crate::AppState,
    toggled: &[CalendarOccurrenceKey],
) {
    for key in toggled {
        if state.retained_completed_calendar_items.contains(key) {
            state.retained_completed_calendar_items.remove(key);
            state.retained_completed.remove(key);
        } else {
            state.retained_completed_calendar_items.insert(key.clone());
            state.retained_completed.insert(key.clone());
        }
    }
}

pub fn past_event_completion_keys(
    workspace: &Workspace,
    now: DateTime<Utc>,
) -> Vec<CalendarOccurrenceKey> {
    let mut keys = Vec::new();

    for scheme in workspace.iter_schemes() {
        for item in &scheme.items {
            if item.kind() != ItemKind::Event {
                continue;
            }
            let (Some(start), Some(end)) = (item.start, item.end) else {
                continue;
            };
            if end > now && item.repeats.is_none() {
                continue;
            }

            let mut from = recurring_completion_scan_start(item, start, end) - Duration::seconds(1);
            from = from.max(now - Duration::days(COMPLETION_LOOKBACK_DAYS));
            let to = now + Duration::seconds(1);
            for occurrence in item.occurrences(from, to) {
                if occurrence.kind != ItemKind::Event {
                    continue;
                }
                if occurrence.end.is_none_or(|end| end > now) || occurrence.state.is_done() {
                    continue;
                }
                keys.push(CalendarOccurrenceKey {
                    scheme_id: scheme.id,
                    item_id: item.id,
                    occurrence: occurrence.id,
                });
            }
        }
    }

    keys.sort_by_key(|key| (key.scheme_id.0, key.item_id.0, key.occurrence.clone()));
    keys.dedup();
    keys
}

pub fn mark_past_event_completion_keys_done(
    workspace: &mut Workspace,
    keys: &[CalendarOccurrenceKey],
    now: DateTime<Utc>,
) -> usize {
    let mut changed = 0;

    for key in keys {
        let Some(scheme) = workspace.scheme_mut(key.scheme_id) else {
            continue;
        };
        let Some(item) = scheme.item_mut(key.item_id) else {
            continue;
        };
        if item.kind() != ItemKind::Event {
            continue;
        }
        let (Some(start), Some(end)) = (item.start, item.end) else {
            continue;
        };
        if end > now && item.repeats.is_none() {
            continue;
        }

        let mut from = recurring_completion_scan_start(item, start, end) - Duration::seconds(1);
        from = from.max(now - Duration::days(COMPLETION_LOOKBACK_DAYS));
        let to = now + Duration::seconds(1);
        let still_due = item.occurrences(from, to).into_iter().any(|occurrence| {
            occurrence.id == key.occurrence
                && occurrence.kind == ItemKind::Event
                && occurrence.end.is_some_and(|end| end <= now)
                && !occurrence.state.is_done()
        });
        if still_due {
            let state = item.state_for_occurrence_mut(key.occurrence.clone());
            if !state.is_done() {
                state.progress = -1;
                changed += 1;
            }
        }
    }

    changed
}

pub fn mark_past_events_done(workspace: &mut Workspace, now: DateTime<Utc>) -> usize {
    let keys = past_event_completion_keys(workspace, now);
    mark_past_event_completion_keys_done(workspace, &keys, now)
}

fn recurring_completion_scan_start(
    item: &Item,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> DateTime<Utc> {
    let mut from = start.min(end);
    let Some(repeats) = item.repeats.as_ref() else {
        return from;
    };

    for rdate in &repeats.rdates {
        from = from.min(rdate.as_utc_lossy());
    }
    for override_ in &repeats.overrides {
        if override_.status == OccurrenceOverrideStatus::Cancelled {
            continue;
        }
        if let OccurrenceId::Recurring { original_start } = &override_.occurrence {
            from = from.min(original_start.as_utc_lossy());
        }
        if let Some(start) = override_.start {
            from = from.min(start);
        }
        if let Some(end) = override_.end {
            from = from.min(end);
        }
        if let Some(available) = override_.available {
            from = from.min(available);
        }
    }

    from
}
