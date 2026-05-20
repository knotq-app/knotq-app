use chrono::{DateTime, TimeZone, Utc};
use knotq_model::{
    CalendarRecurrence, Item, OccurrenceId, OccurrenceOverride, OccurrenceOverrideStatus,
};
use knotq_rrule::{
    recurrence_with_date_override, recurrence_with_prior_overrides, ItemOccurrenceExt,
};

fn dt(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, day, hour, 0, 0).unwrap()
}

#[test]
fn this_event_date_edit_adds_occurrence_override() {
    let recurrence = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=4".to_string()],
        ..CalendarRecurrence::default()
    };
    let occurrence = OccurrenceId::recurring_utc(dt(7, 10));

    let edited = recurrence_with_date_override(
        &recurrence,
        occurrence.clone(),
        true,
        Some(dt(7, 12)),
        true,
        Some(dt(7, 13)),
    );

    assert_eq!(edited.rrules, recurrence.rrules);
    assert_eq!(edited.overrides.len(), 1);
    assert_eq!(edited.overrides[0].occurrence, occurrence);
    assert_eq!(edited.overrides[0].start, Some(dt(7, 12)));
    assert_eq!(edited.overrides[0].end, Some(dt(7, 13)));
}

#[test]
fn all_future_date_edit_preserves_prior_occurrences_and_reduces_count() {
    let recurrence = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=5".to_string()],
        ..CalendarRecurrence::default()
    };
    let mut item = Item::new("standup")
        .with_start(dt(5, 10))
        .with_end(dt(5, 11));
    item.repeats = Some(recurrence.clone());

    let edited = recurrence_with_prior_overrides(
        &item,
        &recurrence,
        &OccurrenceId::recurring_utc(dt(7, 10)),
        2,
    )
    .unwrap();

    assert_eq!(edited.rrules, vec!["FREQ=DAILY;COUNT=3"]);
    assert_eq!(edited.overrides.len(), 2);
    assert_eq!(
        edited
            .overrides
            .iter()
            .map(|override_| override_.occurrence.clone())
            .collect::<Vec<_>>(),
        vec![
            OccurrenceId::recurring_utc(dt(5, 10)),
            OccurrenceId::recurring_utc(dt(6, 10)),
        ]
    );
}

#[test]
fn all_future_date_edit_drops_selected_special_case_override() {
    let selected = OccurrenceId::recurring_utc(dt(7, 10));
    let future = OccurrenceId::recurring_utc(dt(8, 10));
    let recurrence = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=5".to_string()],
        overrides: vec![
            OccurrenceOverride {
                occurrence: selected.clone(),
                status: OccurrenceOverrideStatus::Active,
                start: Some(dt(7, 12)),
                end: Some(dt(7, 13)),
                available: None,
            },
            OccurrenceOverride {
                occurrence: future,
                status: OccurrenceOverrideStatus::Active,
                start: Some(dt(8, 14)),
                end: Some(dt(8, 15)),
                available: None,
            },
        ],
        ..CalendarRecurrence::default()
    };
    let mut item = Item::new("standup")
        .with_start(dt(5, 10))
        .with_end(dt(5, 11));
    item.repeats = Some(recurrence.clone());

    let edited = recurrence_with_prior_overrides(&item, &recurrence, &selected, 2).unwrap();

    assert_eq!(edited.rrules, vec!["FREQ=DAILY;COUNT=3"]);
    assert!(!edited
        .overrides
        .iter()
        .any(|override_| override_.occurrence == selected));
    assert_eq!(edited.overrides.len(), 2);

    let mut moved_item = item.clone();
    moved_item.start = Some(dt(7, 12));
    moved_item.end = Some(dt(7, 13));
    moved_item.repeats = Some(edited);
    let starts = moved_item
        .occurrences(dt(5, 0), dt(11, 0))
        .into_iter()
        .filter_map(|occurrence| occurrence.start)
        .collect::<Vec<_>>();

    assert_eq!(
        starts,
        vec![dt(5, 10), dt(6, 10), dt(7, 12), dt(8, 12), dt(9, 12)]
    );
}
