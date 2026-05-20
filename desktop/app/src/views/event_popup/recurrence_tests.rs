use super::*;
use chrono::TimeZone;
use knotq_model::CalendarDateTime;

#[test]
fn deleting_rrule_recurrence_adds_exdate() {
    let original_start =
        CalendarDateTime::utc(Utc.with_ymd_and_hms(2026, 5, 18, 16, 0, 0).unwrap());
    let repeat = CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE;COUNT=10".to_string()],
        ..Default::default()
    };

    let next = recurrence_without_occurrence(
        &repeat,
        &OccurrenceId::Recurring {
            original_start: original_start.clone(),
        },
    )
    .unwrap();

    assert_eq!(
        next.rrules,
        vec!["FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE;COUNT=10"]
    );
    assert_eq!(next.exdates, vec![original_start]);
    assert!(next.rdates.is_empty());
    assert!(next.overrides.is_empty());
}

#[test]
fn deleting_complex_recurrence_preserves_rule_and_dedupes_exdate() {
    let original_start =
        CalendarDateTime::utc(Utc.with_ymd_and_hms(2026, 5, 18, 16, 0, 0).unwrap());
    let repeat = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;INTERVAL=1".to_string()],
        rdates: Vec::new(),
        exdates: vec![original_start.clone()],
        overrides: Vec::new(),
        raw_import: None,
    };

    let next = recurrence_without_occurrence(
        &repeat,
        &OccurrenceId::Recurring {
            original_start: original_start.clone(),
        },
    )
    .unwrap();

    assert_eq!(next.rrules, vec!["FREQ=DAILY;INTERVAL=1"]);
    assert_eq!(next.exdates, vec![original_start]);
}

#[test]
fn simple_complex_recurrence_remains_editable_with_exdates() {
    let exdate = CalendarDateTime::utc(Utc.with_ymd_and_hms(2026, 5, 20, 16, 0, 0).unwrap());
    let repeat = CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=8".to_string()],
        rdates: Vec::new(),
        exdates: vec![exdate.clone()],
        overrides: Vec::new(),
        raw_import: None,
    };

    assert_eq!(event_repeat_mode(&repeat), Some(EventRepeatMode::Weekly));
    assert_eq!(simple_repeat_end(&repeat), Some(RepeatEnd::Count(8)));

    let next = repeat_with_end(&repeat, RepeatEnd::Never).unwrap();
    assert_eq!(next.rrules, vec!["FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE"]);
    assert_eq!(next.exdates, vec![exdate]);
}

#[test]
fn local_date_repeat_end_roundtrips_without_timezone_shift() {
    let date = NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
    let until = local_repeat_until_for_date(date).unwrap();
    let repeat = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;INTERVAL=1".to_string()],
        ..Default::default()
    };

    let next = repeat_with_end(&repeat, RepeatEnd::Until(until)).unwrap();

    assert_eq!(
        next.rrules,
        vec![format!(
            "FREQ=DAILY;INTERVAL=1;UNTIL={}",
            until.format("%Y%m%dT%H%M%SZ")
        )]
    );
    assert_eq!(simple_repeat_end(&next), Some(RepeatEnd::Until(until)));
    assert_eq!(
        match simple_repeat_end(&next) {
            Some(RepeatEnd::Until(until)) => until.with_timezone(&Local).date_naive(),
            other => panic!("expected repeat end, got {other:?}"),
        },
        date
    );
}

#[test]
fn deleting_this_and_future_sets_until_before_occurrence() {
    let original_start =
        CalendarDateTime::utc(Utc.with_ymd_and_hms(2026, 5, 21, 16, 0, 0).unwrap());
    let repeat = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;INTERVAL=1".to_string()],
        ..Default::default()
    };

    let next = recurrence_without_this_and_future(
        &repeat,
        &OccurrenceId::Recurring {
            original_start: original_start.clone(),
        },
        4,
    )
    .unwrap()
    .unwrap();

    let simple = editable_simple_recurrence(&next).unwrap();
    let SimpleRecurrence::Daily {
        end: RepeatEnd::Until(until),
        ..
    } = simple
    else {
        panic!("future deletion should truncate the daily recurrence");
    };
    assert_eq!(until, original_start.as_utc_lossy() - Duration::seconds(1));
}
