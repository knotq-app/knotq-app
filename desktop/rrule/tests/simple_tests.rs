use chrono::{TimeZone, Utc};
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
