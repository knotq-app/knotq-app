use chrono::{Duration, TimeZone, Utc};
use knotq_model::{
    CalendarRecurrence, Item, OccurrenceId, OccurrenceOverride, OccurrenceOverrideStatus,
};
use knotq_state::{
    mark_past_event_completion_keys_done, mark_past_events_done, past_event_completion_keys,
};

mod support;

use support::{read_only_workspace_with_item, workspace_with_item};

#[test]
fn mark_past_events_done_completes_elapsed_events() {
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
    let end = start + Duration::hours(1);
    let mut workspace = workspace_with_item(Item::new("Class").with_start(start).with_end(end));

    let changed = mark_past_events_done(&mut workspace, end + Duration::minutes(1));

    let item = &workspace.iter_schemes().next().unwrap().items[0];
    assert_eq!(changed, 1);
    assert!(item.single_state().is_done());
}

#[test]
fn past_event_completion_keys_can_be_applied_after_background_scan() {
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
    let end = start + Duration::hours(1);
    let mut workspace = workspace_with_item(Item::new("Class").with_start(start).with_end(end));
    let now = end + Duration::minutes(1);

    let keys = past_event_completion_keys(&workspace, now);
    let changed = mark_past_event_completion_keys_done(&mut workspace, &keys, now);

    let item = &workspace.iter_schemes().next().unwrap().items[0];
    assert_eq!(keys.len(), 1);
    assert_eq!(changed, 1);
    assert!(item.single_state().is_done());
}

#[test]
fn mark_past_events_done_completes_recent_elapsed_read_only_events() {
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
    let end = start + Duration::hours(1);
    let mut workspace =
        read_only_workspace_with_item(Item::new("Imported class").with_start(start).with_end(end));

    let changed = mark_past_events_done(&mut workspace, end + Duration::minutes(1));

    let item = &workspace.iter_schemes().next().unwrap().items[0];
    assert_eq!(changed, 1);
    assert!(item.single_state().is_done());
}

#[test]
fn mark_past_events_done_skips_old_events() {
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
    let end = start + Duration::hours(1);
    let mut workspace = workspace_with_item(Item::new("Old class").with_start(start).with_end(end));

    let changed = mark_past_events_done(&mut workspace, end + Duration::days(8));

    let item = &workspace.iter_schemes().next().unwrap().items[0];
    assert_eq!(changed, 0);
    assert!(!item.single_state().is_done());
}

#[test]
fn mark_past_events_done_completes_elapsed_recurring_occurrences() {
    let start = Utc.with_ymd_and_hms(2026, 5, 18, 16, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 18, 18, 0, 0).unwrap();
    let mut item = Item::new("Class").with_start(start).with_end(end);
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;BYDAY=MO,WE,FR".into()],
        ..Default::default()
    });
    let mut workspace = workspace_with_item(item);

    let changed = mark_past_events_done(
        &mut workspace,
        Utc.with_ymd_and_hms(2026, 5, 18, 19, 0, 0).unwrap(),
    );

    let item = &workspace.iter_schemes().next().unwrap().items[0];
    assert_eq!(changed, 1);
    assert!(item
        .state_for_occurrence(&OccurrenceId::recurring_utc(start))
        .is_done());
}

#[test]
fn mark_past_events_done_completes_elapsed_overridden_recurring_occurrences() {
    let base_start = Utc.with_ymd_and_hms(2026, 5, 20, 19, 30, 0).unwrap();
    let base_end = Utc.with_ymd_and_hms(2026, 5, 20, 20, 15, 0).unwrap();
    let original_start = Utc.with_ymd_and_hms(2026, 5, 20, 19, 30, 0).unwrap();
    let override_start = Utc.with_ymd_and_hms(2026, 5, 18, 16, 0, 0).unwrap();
    let override_end = Utc.with_ymd_and_hms(2026, 5, 18, 18, 0, 0).unwrap();
    let occurrence = OccurrenceId::recurring_utc(original_start);
    let mut item = Item::new("Class").with_start(base_start).with_end(base_end);
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;BYDAY=WE,TH,FR".into()],
        overrides: vec![OccurrenceOverride {
            occurrence: occurrence.clone(),
            status: OccurrenceOverrideStatus::Active,
            start: Some(override_start),
            end: Some(override_end),
            available: None,
        }],
        ..Default::default()
    });
    let mut workspace = workspace_with_item(item);

    let changed = mark_past_events_done(
        &mut workspace,
        Utc.with_ymd_and_hms(2026, 5, 18, 19, 0, 0).unwrap(),
    );

    let item = &workspace.iter_schemes().next().unwrap().items[0];
    assert_eq!(changed, 1);
    assert!(item.state_for_occurrence(&occurrence).is_done());
}
