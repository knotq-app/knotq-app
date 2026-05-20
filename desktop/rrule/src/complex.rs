use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};

use knotq_model::{
    CalendarDateTime, CalendarRecurrence, Item, ItemKind, ItemMarker, Occurrence, OccurrenceId,
    OccurrenceOverrideStatus, RepeatEnd, SimpleRecurrence,
};

use crate::expand::{materialize_occurrence, occurrence_hits_range, occurrence_sort_key};
use crate::ical::{parse_rrule_fields, parse_rrule_until, parse_rrule_weekdays};
use crate::overrides::apply_override;
use crate::simple::expand_simple;

pub(crate) fn expand_complex(
    item: &Item,
    kind: ItemKind,
    anchor: DateTime<Utc>,
    recurrence: &CalendarRecurrence,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<Occurrence> {
    let mut anchors = BTreeSet::new();
    if recurrence.rrules.is_empty() && recurrence.rdates.is_empty() {
        anchors.insert(anchor);
    }
    for rdate in &recurrence.rdates {
        anchors.insert(rdate.as_utc_lossy());
    }
    for rrule in &recurrence.rrules {
        anchors.extend(expand_rrule_anchors(anchor, rrule, from, to));
    }

    let exdates = recurrence
        .exdates
        .iter()
        .map(CalendarDateTime::as_utc_lossy)
        .collect::<BTreeSet<_>>();
    let overrides = recurrence
        .overrides
        .iter()
        .map(|override_| (override_.occurrence.clone(), override_))
        .collect::<BTreeMap<_, _>>();

    let mut out = Vec::new();
    let mut index = 0usize;
    let mut seen = BTreeSet::new();
    for current in anchors {
        let occurrence_id = OccurrenceId::recurring_utc(current);
        seen.insert(occurrence_id.clone());
        if exdates.contains(&current) {
            continue;
        }
        if overrides
            .get(&occurrence_id)
            .is_some_and(|override_| override_.status == OccurrenceOverrideStatus::Cancelled)
        {
            continue;
        }
        let mut occurrence =
            materialize_occurrence(item, kind, occurrence_id.clone(), index, anchor, current);
        if let Some(override_) = overrides.get(&occurrence_id) {
            apply_override(&mut occurrence, item, override_);
        }
        let effective_anchor = occurrence_sort_key(&occurrence).unwrap_or(current);
        if occurrence_hits_range(occurrence.start, occurrence.end, effective_anchor, from, to) {
            out.push(occurrence);
        }
        index += 1;
    }

    for override_ in &recurrence.overrides {
        if override_.status == OccurrenceOverrideStatus::Cancelled
            || seen.contains(&override_.occurrence)
        {
            continue;
        }
        let original_anchor = match &override_.occurrence {
            OccurrenceId::Single => anchor,
            OccurrenceId::Recurring { original_start } => original_start.as_utc_lossy(),
        };
        let mut occurrence = materialize_occurrence(
            item,
            kind,
            override_.occurrence.clone(),
            index,
            anchor,
            original_anchor,
        );
        apply_override(&mut occurrence, item, override_);
        let effective_anchor = occurrence_sort_key(&occurrence).unwrap_or(original_anchor);
        if occurrence_hits_range(occurrence.start, occurrence.end, effective_anchor, from, to) {
            out.push(occurrence);
        }
        index += 1;
    }

    out
}

pub(crate) fn expand_rrule_anchors(
    anchor: DateTime<Utc>,
    raw_rule: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    let fields = parse_rrule_fields(raw_rule);
    let Some(freq) = fields.get("FREQ").map(String::as_str) else {
        return Vec::new();
    };
    let interval = fields
        .get("INTERVAL")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let end = fields
        .get("COUNT")
        .and_then(|value| value.parse::<usize>().ok())
        .map(RepeatEnd::Count)
        .or_else(|| {
            fields
                .get("UNTIL")
                .and_then(|value| parse_rrule_until(value))
                .map(RepeatEnd::Until)
        })
        .unwrap_or(RepeatEnd::Never);
    let simple = match freq {
        "DAILY" => SimpleRecurrence::Daily { interval, end },
        "WEEKLY" => SimpleRecurrence::Weekly {
            interval,
            weekdays: fields
                .get("BYDAY")
                .map(|value| parse_rrule_weekdays(value))
                .unwrap_or_default(),
            end,
        },
        "MONTHLY" => SimpleRecurrence::Monthly { interval, end },
        "YEARLY" => SimpleRecurrence::Yearly { interval, end },
        _ => return Vec::new(),
    };
    let mut item = Item::new("");
    item.marker = ItemMarker::Checkbox;
    item.start = Some(anchor);
    expand_simple(&item, ItemKind::Reminder, anchor, &simple, from, to)
        .into_iter()
        .filter_map(|occ| occ.start)
        .collect()
}
