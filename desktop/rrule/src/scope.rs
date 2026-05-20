use chrono::{DateTime, Duration, Utc};
use knotq_model::{Item, OccurrenceId, OccurrenceOverride, OccurrenceOverrideStatus, Recurrence};

use crate::ItemOccurrenceExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecurrenceEditScope {
    ThisEvent,
    AllFuture,
    AllEvents,
}

pub fn scoped_date_edit_recurrence(
    item: &Item,
    recurrence: &Recurrence,
    occurrence: OccurrenceId,
    occurrence_index: usize,
    start_dirty: bool,
    draft_start: Option<DateTime<Utc>>,
    end_dirty: bool,
    draft_end: Option<DateTime<Utc>>,
    scope: RecurrenceEditScope,
) -> Option<Recurrence> {
    match scope {
        RecurrenceEditScope::ThisEvent => Some(recurrence_with_date_override(
            recurrence,
            occurrence,
            start_dirty,
            draft_start,
            end_dirty,
            draft_end,
        )),
        RecurrenceEditScope::AllFuture => {
            recurrence_with_prior_overrides(item, recurrence, &occurrence, occurrence_index)
        }
        RecurrenceEditScope::AllEvents => None,
    }
}

pub fn recurrence_with_date_override(
    recurrence: &Recurrence,
    occurrence: OccurrenceId,
    start_dirty: bool,
    start: Option<DateTime<Utc>>,
    end_dirty: bool,
    end: Option<DateTime<Utc>>,
) -> Recurrence {
    let mut next = recurrence.clone();
    let mut override_ = next
        .overrides
        .iter()
        .find(|override_| override_.occurrence == occurrence)
        .cloned()
        .unwrap_or(OccurrenceOverride {
            occurrence: occurrence.clone(),
            status: OccurrenceOverrideStatus::Active,
            start: None,
            end: None,
            available: None,
        });
    override_.status = OccurrenceOverrideStatus::Active;
    if start_dirty {
        override_.start = start;
    }
    if end_dirty {
        override_.end = end;
    }
    upsert_occurrence_override(&mut next, override_);
    next
}

pub fn recurrence_with_prior_overrides(
    item: &Item,
    recurrence: &Recurrence,
    occurrence: &OccurrenceId,
    occurrence_index: usize,
) -> Option<Recurrence> {
    let OccurrenceId::Recurring { original_start } = occurrence else {
        return None;
    };
    let selected_anchor = original_start.as_utc_lossy();
    let anchor = item.start.or(item.end).or(item.available)?;
    let mut next = recurrence.clone();
    adjust_rrule_counts_for_future_edit(&mut next, occurrence_index);
    next.rdates
        .retain(|date| date.as_utc_lossy() < selected_anchor);
    next.exdates
        .retain(|date| date.as_utc_lossy() < selected_anchor);
    next.overrides.retain(|override_| {
        occurrence_id_anchor(&override_.occurrence).is_some_and(|anchor| anchor < selected_anchor)
    });

    let from = anchor - Duration::seconds(1);
    for occurrence in item.occurrences(from, selected_anchor) {
        if occurrence.occurrence_index >= occurrence_index {
            continue;
        }
        upsert_occurrence_override(
            &mut next,
            OccurrenceOverride {
                occurrence: occurrence.id,
                status: OccurrenceOverrideStatus::Active,
                start: occurrence.start,
                end: occurrence.end,
                available: occurrence.available,
            },
        );
    }
    Some(next)
}

fn occurrence_id_anchor(occurrence: &OccurrenceId) -> Option<DateTime<Utc>> {
    match occurrence {
        OccurrenceId::Single => None,
        OccurrenceId::Recurring { original_start } => Some(original_start.as_utc_lossy()),
    }
}

pub fn adjust_rrule_counts_for_future_edit(recurrence: &mut Recurrence, prior_count: usize) {
    if prior_count == 0 {
        return;
    }
    for rrule in &mut recurrence.rrules {
        let mut changed = false;
        let mut parts = Vec::new();
        for part in rrule.split(';') {
            let Some((key, value)) = part.split_once('=') else {
                parts.push(part.to_string());
                continue;
            };
            if key.eq_ignore_ascii_case("COUNT") {
                if let Ok(count) = value.parse::<usize>() {
                    parts.push(format!(
                        "COUNT={}",
                        count.saturating_sub(prior_count).max(1)
                    ));
                    changed = true;
                    continue;
                }
            }
            parts.push(part.to_string());
        }
        if changed {
            *rrule = parts.join(";");
        }
    }
}

fn upsert_occurrence_override(recurrence: &mut Recurrence, override_: OccurrenceOverride) {
    if let Some(slot) = recurrence
        .overrides
        .iter_mut()
        .find(|existing| existing.occurrence == override_.occurrence)
    {
        *slot = override_;
    } else {
        recurrence.overrides.push(override_);
    }
}
