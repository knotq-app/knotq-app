use chrono::{Duration, TimeZone, Utc};
use knotq_model::{
    CalendarRecurrence, Item, ItemId, OccurrenceId, OccurrenceOverride, OccurrenceOverrideStatus,
    SchemeId,
};
use knotq_state::{
    mark_past_event_completion_keys_done, mark_past_events_done, past_event_completion_keys,
    CalendarOccurrenceKey, RetainedCompletedItems, RETAINED_COMPLETED_TTL_SECS,
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

fn occurrence_key() -> CalendarOccurrenceKey {
    CalendarOccurrenceKey {
        scheme_id: SchemeId::new(),
        item_id: ItemId::new(),
        occurrence: OccurrenceId::Single,
    }
}

#[test]
fn retained_completed_items_hold_their_place_until_the_ttl_elapses() {
    let completed_at = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
    let key = occurrence_key();
    let mut retained = RetainedCompletedItems::default();
    retained.insert(key.clone(), completed_at);

    // Freshly completed: the row keeps its place (no layout shift)...
    assert!(retained.is_retained(&key, completed_at));
    assert!(retained.is_retained(
        &key,
        completed_at + Duration::seconds(RETAINED_COMPLETED_TTL_SECS - 1)
    ));
    // ...but not forever: once the TTL elapses it reads as gone, even before a
    // purge sweeps the entry out.
    assert!(!retained.is_retained(
        &key,
        completed_at + Duration::seconds(RETAINED_COMPLETED_TTL_SECS)
    ));
    assert!(!retained.is_retained(&key, completed_at + Duration::days(2)));
    // Unknown keys were never retained.
    assert!(!retained.is_retained(&occurrence_key(), completed_at));
}

#[test]
fn retained_completed_purge_drops_only_expired_entries() {
    let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
    let fresh = occurrence_key();
    let stale = occurrence_key();
    let mut retained = RetainedCompletedItems::default();
    retained.insert(fresh.clone(), now - Duration::minutes(5));
    retained.insert(
        stale.clone(),
        now - Duration::seconds(RETAINED_COMPLETED_TTL_SECS + 1),
    );

    assert_eq!(retained.purge_expired(now), 1);
    assert!(retained.contains(&fresh));
    assert!(!retained.contains(&stale));
    assert_eq!(retained.purge_expired(now), 0, "purge is idempotent");
}

#[test]
fn retained_completed_next_expiry_tracks_the_earliest_completion() {
    let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
    let ttl = Duration::seconds(RETAINED_COMPLETED_TTL_SECS);
    let mut retained = RetainedCompletedItems::default();
    assert_eq!(retained.next_expiry(), None);

    let early = occurrence_key();
    retained.insert(occurrence_key(), now);
    retained.insert(early.clone(), now - Duration::minutes(30));
    assert_eq!(
        retained.next_expiry(),
        Some(now - Duration::minutes(30) + ttl),
        "the timeline task must wake when the OLDEST completion ages out"
    );

    // Re-completing an entry restarts its TTL.
    retained.insert(early, now);
    assert_eq!(retained.next_expiry(), Some(now + ttl));
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
