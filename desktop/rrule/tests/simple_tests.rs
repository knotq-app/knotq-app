use chrono::{Duration, TimeZone, Utc};
use knotq_date_util::DateRange;
use knotq_model::{CalendarRecurrence, Item};
use knotq_rrule::{expand_item, OccurrenceExpander};

#[test]
fn daily_recurrence_expands_in_range() {
    let recurrence = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=3".to_string()],
        ..Default::default()
    };
    let item = Item::new("standup")
        .with_start(Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap())
        .with_repeats(recurrence);
    let range = DateRange {
        start: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap(),
    };
    assert_eq!(expand_item(&item, range).len(), 3);
    assert!(knotq_rrule::DefaultExpander
        .next_after(&item, range.start)
        .is_some());
}

#[test]
fn daily_recurrence_jumps_to_query_window() {
    let anchor = Utc.with_ymd_and_hms(2020, 1, 1, 9, 0, 0).unwrap();
    let target = Utc.with_ymd_and_hms(2026, 1, 10, 9, 0, 0).unwrap();
    let item = Item::new("standup")
        .with_start(anchor)
        .with_repeats(CalendarRecurrence {
            rrules: vec!["FREQ=DAILY".to_string()],
            ..Default::default()
        });
    let range = DateRange {
        start: Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 1, 11, 0, 0, 0).unwrap(),
    };

    let occs = expand_item(&item, range);

    assert_eq!(occs.len(), 1);
    assert_eq!(occs[0].start, Some(target));
}

#[test]
fn daily_count_before_query_window_expands_empty() {
    let item = Item::new("standup")
        .with_start(Utc.with_ymd_and_hms(2020, 1, 1, 9, 0, 0).unwrap())
        .with_repeats(CalendarRecurrence {
            rrules: vec!["FREQ=DAILY;COUNT=3".to_string()],
            ..Default::default()
        });
    let range = DateRange {
        start: Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 1, 11, 0, 0, 0).unwrap(),
    };

    assert!(expand_item(&item, range).is_empty());
}

#[test]
fn prev_before_only_searches_recent_occurrences() {
    let before = Utc.with_ymd_and_hms(2026, 1, 10, 9, 0, 0).unwrap();
    let item = Item::new("old reminder").with_start(before - Duration::days(8));

    assert!(knotq_rrule::DefaultExpander
        .prev_before(&item, before)
        .is_none());
}
