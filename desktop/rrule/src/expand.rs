use chrono::{DateTime, Utc};

use knotq_model::{Item, ItemKind, Occurrence, OccurrenceId};

use crate::complex::expand_complex;

pub trait ItemOccurrenceExt {
    fn occurrences(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Occurrence>;
}

impl ItemOccurrenceExt for Item {
    fn occurrences(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Occurrence> {
        let kind = self.kind();
        let Some(anchor) = occurrence_anchor(self) else {
            return vec![Occurrence {
                id: OccurrenceId::Single,
                start: None,
                end: None,
                available: None,
                kind,
                occurrence_index: 0,
                state: self.single_state(),
            }];
        };

        let Some(recurrence) = self.repeats.as_ref() else {
            if !occurrence_hits_range(self.start, self.end, anchor, from, to) {
                return Vec::new();
            }
            return vec![Occurrence {
                id: OccurrenceId::Single,
                start: self.start,
                end: self.end,
                available: self.available,
                kind,
                occurrence_index: 0,
                state: self.single_state(),
            }];
        };

        let mut occurrences = expand_complex(self, kind, anchor, recurrence, from, to);
        occurrences.sort_by_key(|occ| occurrence_sort_key(occ).unwrap_or(anchor));
        occurrences
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ExpansionCtx<'a> {
    pub(crate) item: &'a Item,
    pub(crate) kind: ItemKind,
    pub(crate) anchor: DateTime<Utc>,
    pub(crate) from: DateTime<Utc>,
    pub(crate) to: DateTime<Utc>,
}

pub(crate) fn occurrence_anchor(item: &Item) -> Option<DateTime<Utc>> {
    item.start.or(item.end).or(item.available)
}

pub(crate) fn occurrence_sort_key(occurrence: &Occurrence) -> Option<DateTime<Utc>> {
    occurrence.start.or(occurrence.end).or(occurrence.available)
}

pub(crate) fn occurrence_hits_range(
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    anchor: DateTime<Utc>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> bool {
    match (start, end) {
        (Some(s), Some(e)) => s < to && e >= from,
        _ => anchor >= from && anchor < to,
    }
}

pub(crate) fn materialize_occurrence(
    item: &Item,
    kind: ItemKind,
    id: OccurrenceId,
    occurrence_index: usize,
    original_anchor: DateTime<Utc>,
    current_anchor: DateTime<Utc>,
) -> Occurrence {
    let delta = current_anchor - original_anchor;
    let state = item.state_for_occurrence(&id);
    Occurrence {
        id,
        start: item.start.map(|start| start + delta),
        end: item.end.map(|end| end + delta),
        available: item.available.map(|available| available + delta),
        kind,
        occurrence_index,
        state,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use knotq_model::{CalendarRecurrence, Item, OccurrenceId};

    use super::*;

    #[test]
    fn non_recurring_event_in_range() {
        let mut item = Item::new("meeting");
        item.start = Some(Utc.with_ymd_and_hms(2026, 1, 5, 10, 0, 0).unwrap());
        item.end = Some(Utc.with_ymd_and_hms(2026, 1, 5, 11, 0, 0).unwrap());
        let occs = item.occurrences(
            Utc.with_ymd_and_hms(2026, 1, 4, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 6, 0, 0, 0).unwrap(),
        );
        assert_eq!(occs.len(), 1);
        assert_eq!(occs[0].id, OccurrenceId::Single);
    }

    #[test]
    fn weekly_rrule_repeat_three_days_per_week() {
        let mut item = Item::new("MATH 15");
        item.start = Some(Utc.with_ymd_and_hms(2026, 1, 5, 18, 0, 0).unwrap());
        item.end = Some(Utc.with_ymd_and_hms(2026, 1, 5, 19, 0, 0).unwrap());
        item.repeats = Some(CalendarRecurrence {
            rrules: vec!["FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE,FR".to_string()],
            ..Default::default()
        });
        let occs = item.occurrences(
            Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 12, 0, 0, 0).unwrap(),
        );
        assert_eq!(occs.len(), 3);
        assert_eq!(
            occs[1].id,
            OccurrenceId::recurring_utc(Utc.with_ymd_and_hms(2026, 1, 7, 18, 0, 0).unwrap())
        );
    }

    #[test]
    fn weekly_rrule_without_byday_repeats_on_anchor_weekday() {
        let mut item = Item::new("review");
        item.start = Some(Utc.with_ymd_and_hms(2026, 1, 8, 8, 30, 0).unwrap());
        item.repeats = Some(CalendarRecurrence {
            rrules: vec!["FREQ=WEEKLY;INTERVAL=1".to_string()],
            ..Default::default()
        });

        let occs = item.occurrences(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 23, 0, 0, 0).unwrap(),
        );

        assert_eq!(occs.len(), 3);
        assert_eq!(
            occs.iter().filter_map(|occ| occ.start).collect::<Vec<_>>(),
            vec![
                Utc.with_ymd_and_hms(2026, 1, 8, 8, 30, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 1, 15, 8, 30, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 1, 22, 8, 30, 0).unwrap(),
            ]
        );
    }

    #[test]
    fn repeat_count_is_total_occurrences() {
        let mut item = Item::new("class");
        item.start = Some(Utc.with_ymd_and_hms(2026, 1, 5, 18, 0, 0).unwrap());
        item.repeats = Some(CalendarRecurrence {
            rrules: vec!["FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE,FR;COUNT=4".to_string()],
            ..Default::default()
        });

        let occs = item.occurrences(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 31, 0, 0, 0).unwrap(),
        );

        assert_eq!(occs.len(), 4);
        assert_eq!(occs[0].occurrence_index, 0);
        assert_eq!(occs[3].occurrence_index, 3);
        assert_eq!(
            occs[3].id,
            OccurrenceId::recurring_utc(Utc.with_ymd_and_hms(2026, 1, 12, 18, 0, 0).unwrap())
        );
    }

    #[test]
    fn date_only_until_includes_occurrences_on_that_date() {
        let mut item = Item::new("class");
        item.start = Some(Utc.with_ymd_and_hms(2026, 1, 5, 18, 0, 0).unwrap());
        item.repeats = Some(CalendarRecurrence {
            rrules: vec!["FREQ=DAILY;INTERVAL=1;UNTIL=20260107".to_string()],
            ..Default::default()
        });

        let occs = item.occurrences(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap(),
        );

        assert_eq!(occs.len(), 3);
        assert_eq!(
            occs[2].id,
            OccurrenceId::recurring_utc(Utc.with_ymd_and_hms(2026, 1, 7, 18, 0, 0).unwrap())
        );
    }

    #[test]
    fn recurring_occurrence_identity_is_stable_across_query_windows() {
        let mut item = Item::new("class");
        item.start = Some(Utc.with_ymd_and_hms(2026, 1, 5, 18, 0, 0).unwrap());
        item.end = Some(Utc.with_ymd_and_hms(2026, 1, 5, 19, 0, 0).unwrap());
        item.repeats = Some(CalendarRecurrence {
            rrules: vec!["FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE,FR".to_string()],
            ..Default::default()
        });

        let wed_start = Utc.with_ymd_and_hms(2026, 1, 7, 0, 0, 0).unwrap();
        let wed_end = Utc.with_ymd_and_hms(2026, 1, 8, 0, 0, 0).unwrap();
        let week_start = Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap();
        let week_end = Utc.with_ymd_and_hms(2026, 1, 12, 0, 0, 0).unwrap();

        let day_id = item.occurrences(wed_start, wed_end)[0].id.clone();
        let week_id = item
            .occurrences(week_start, week_end)
            .into_iter()
            .find(|occ| occ.start == Some(Utc.with_ymd_and_hms(2026, 1, 7, 18, 0, 0).unwrap()))
            .unwrap()
            .id;
        assert_eq!(day_id, week_id);
    }
}
