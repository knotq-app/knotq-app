use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use knotq_model::{Item, ItemKind, OccurrenceId, OccurrenceOverrideStatus, SchemeId, Workspace};
use knotq_rrule::ItemOccurrenceExt;

const COMPLETION_LOOKBACK_DAYS: i64 = 7;

/// How long a completed overdue occurrence keeps its place on the upcoming/overdue
/// panel after being checked off. Long enough that checking a row off never yanks
/// it out from under the pointer (no layout shift), short enough that finished
/// work doesn't haunt the panel for the rest of the session.
pub const RETAINED_COMPLETED_TTL_SECS: i64 = 3600;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CalendarOccurrenceKey {
    pub scheme_id: SchemeId,
    pub item_id: knotq_model::ItemId,
    pub occurrence: OccurrenceId,
}

/// Overdue occurrences that were completed this session and should stay visible
/// (faded, in place) on the upcoming/overdue panel — each with its completion
/// time, so retention expires after [`RETAINED_COMPLETED_TTL_SECS`].
#[derive(Clone, Debug, Default)]
pub struct RetainedCompletedItems {
    completed_at: HashMap<CalendarOccurrenceKey, DateTime<Utc>>,
}

impl RetainedCompletedItems {
    pub fn contains(&self, key: &CalendarOccurrenceKey) -> bool {
        self.completed_at.contains_key(key)
    }

    /// Record `key` as completed at `completed_at`; re-inserting restarts the TTL.
    pub fn insert(&mut self, key: CalendarOccurrenceKey, completed_at: DateTime<Utc>) {
        self.completed_at.insert(key, completed_at);
    }

    pub fn remove(&mut self, key: &CalendarOccurrenceKey) {
        self.completed_at.remove(key);
    }

    /// Whether `key` should still hold its place on the panel at `now`. False for
    /// unknown keys AND for completions older than the TTL — expired entries read
    /// as gone even before [`Self::purge_expired`] sweeps them out.
    pub fn is_retained(&self, key: &CalendarOccurrenceKey, now: DateTime<Utc>) -> bool {
        self.completed_at
            .get(key)
            .is_some_and(|at| now.signed_duration_since(*at) < retention_ttl())
    }

    /// When the earliest-completed entry expires — the wakeup deadline for a
    /// timeline task that wants to re-render right as a row ages out.
    pub fn next_expiry(&self) -> Option<DateTime<Utc>> {
        self.completed_at.values().min().map(|at| *at + retention_ttl())
    }

    /// Drop entries past the TTL; returns how many were removed (non-zero means
    /// the panel contents changed and a re-render is due).
    pub fn purge_expired(&mut self, now: DateTime<Utc>) -> usize {
        let before = self.completed_at.len();
        self.completed_at
            .retain(|_, at| now.signed_duration_since(*at) < retention_ttl());
        before - self.completed_at.len()
    }

    pub fn clear(&mut self) {
        self.completed_at.clear();
    }
}

fn retention_ttl() -> Duration {
    Duration::seconds(RETAINED_COMPLETED_TTL_SECS)
}

pub fn complete_past_events(state: &mut crate::AppState, now: DateTime<Utc>) -> usize {
    let changed = mark_past_events_done(&mut state.workspace, now);
    if changed > 0 {
        let all_ids: Vec<_> = state.workspace.schemes.keys().copied().collect();
        for id in all_ids {
            state.dirty_schemes.insert(id);
        }
        state.index_dirty = true;
        state.mark_direct_workspace_dirty();
    }
    changed
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
