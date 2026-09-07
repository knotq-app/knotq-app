use chrono::{Datelike, Duration, Local, TimeZone, Timelike, Utc, Weekday};
use knotq_date_util::DateRange;
use knotq_model::{CalendarRecurrence, Item, RepeatWeekday};
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

#[test]
fn weekly_recurrence_keeps_local_weekday_for_late_evening_events() {
    let local_anchor = Local
        .with_ymd_and_hms(2026, 1, 5, 21, 0, 0)
        .single()
        .expect("test date must be representable in the local timezone");
    assert_eq!(local_anchor.weekday(), Weekday::Mon);
    let anchor = local_anchor.with_timezone(&Utc);
    let item = Item::new("evening class")
        .with_start(anchor)
        .with_repeats(CalendarRecurrence {
            rrules: vec!["FREQ=WEEKLY;BYDAY=MO;COUNT=2".to_string()],
            ..Default::default()
        });
    let range = DateRange {
        start: anchor,
        end: anchor + Duration::weeks(2),
    };

    let occurrences = expand_item(&item, range);

    assert_eq!(occurrences.len(), 2);
    for occurrence in occurrences {
        let local = occurrence.start.unwrap().with_timezone(&Local);
        assert_eq!(local.weekday(), Weekday::Mon);
        assert_eq!(local.hour(), 21);
    }
}

#[test]
fn weekly_recurrence_keeps_all_selected_local_weekdays() {
    let local_anchor = Local
        .with_ymd_and_hms(2026, 1, 5, 21, 0, 0)
        .single()
        .expect("test date must be representable in the local timezone");
    let anchor = local_anchor.with_timezone(&Utc);
    let item = Item::new("evening classes")
        .with_start(anchor)
        .with_repeats(CalendarRecurrence {
            rrules: vec!["FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=3".to_string()],
            ..Default::default()
        });

    let occurrences = expand_item(
        &item,
        DateRange {
            start: anchor,
            end: anchor + Duration::weeks(1),
        },
    );

    assert_eq!(occurrences.len(), 3);
    assert_eq!(
        occurrences
            .iter()
            .map(|occurrence| occurrence.start.unwrap().with_timezone(&Local).weekday())
            .collect::<Vec<_>>(),
        vec![Weekday::Mon, Weekday::Wed, Weekday::Fri]
    );
    assert!(occurrences
        .iter()
        .all(|occurrence| { occurrence.start.unwrap().with_timezone(&Local).hour() == 21 }));
}

#[test]
fn default_weekday_uses_local_date_for_late_evening_event() {
    let local_anchor = Local
        .with_ymd_and_hms(2026, 1, 5, 21, 0, 0)
        .single()
        .expect("test date must be representable in the local timezone");
    let item = Item::new("evening class").with_start(local_anchor.with_timezone(&Utc));

    assert_eq!(
        knotq_rrule::weekday_util::default_weekday_for_item(&item),
        RepeatWeekday::Mon
    );
}

#[test]
fn weekly_recurrence_preserves_wall_clock_time_across_dst_boundary() {
    let local_anchor = Local
        .with_ymd_and_hms(2026, 3, 2, 21, 0, 0)
        .single()
        .expect("test date must be representable in the local timezone");
    let anchor = local_anchor.with_timezone(&Utc);
    let item = Item::new("DST-spanning class")
        .with_start(anchor)
        .with_repeats(CalendarRecurrence {
            rrules: vec!["FREQ=WEEKLY;BYDAY=MO;COUNT=2".to_string()],
            ..Default::default()
        });

    let occurrences = expand_item(
        &item,
        DateRange {
            start: anchor,
            end: anchor + Duration::weeks(2),
        },
    );

    assert_eq!(occurrences.len(), 2);
    let local_times = occurrences
        .iter()
        .map(|occurrence| occurrence.start.unwrap().with_timezone(&Local))
        .collect::<Vec<_>>();
    assert_eq!(local_times[0].date_naive(), local_anchor.date_naive());
    assert_eq!(
        local_times[1].date_naive(),
        local_anchor.date_naive() + Duration::days(7)
    );
    assert!(local_times.iter().all(|datetime| datetime.hour() == 21));
}
