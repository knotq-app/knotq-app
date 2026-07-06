use chrono::{Duration, NaiveDate, TimeZone, Utc};
use knotq_date_util::DateRange;
use knotq_model::{
    daily_queue_scheme_id, CalendarRecurrence, Item, ItemKind, NodeRef, Occurrence, OccurrenceId,
    Scheme, Workspace,
};
use knotq_notifications::{
    completed_notification_keys, compute_due_notifications,
    compute_due_notifications_with_expander, compute_due_notifications_with_lead_times,
    expired_event_notification_keys, notification_keys_for_item, notification_keys_for_occurrence,
    NotificationKind, NotificationLeadTimes,
};
use knotq_rrule::OccurrenceExpander;

#[test]
fn event_notifications_fire_at_start_time() {
    let start = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 10, 13, 0, 0).unwrap();
    let workspace = workspace_with_item(Item::new("Class").with_start(start).with_end(end));

    let notes = compute_due_notifications_with_lead_times(
        &workspace,
        NotificationLeadTimes {
            event_offset_secs: 0,
            ..NotificationLeadTimes::default()
        },
        Utc.with_ymd_and_hms(2026, 5, 10, 11, 59, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 10, 12, 1, 0).unwrap(),
    );

    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].kind, NotificationKind::Event);
    assert_eq!(notes[0].fire_at, start);
    assert_eq!(notes[0].expires_at, Some(end));
    assert_eq!(notes[0].end_at, Some(end));
    assert_eq!(notes[0].title, "Class");
    assert!(!notes[0].body.starts_with("From "));
    assert!(notes[0].body.contains(" to "));
}

#[test]
fn assignment_notification_uses_default_due_lead_time() {
    let due = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let fire_at = due - Duration::hours(2);
    let workspace = workspace_with_item(Item::new("Essay").with_end(due));

    let notes = compute_due_notifications_with_lead_times(
        &workspace,
        NotificationLeadTimes {
            assignment_offset_secs: 2 * 60 * 60,
            ..NotificationLeadTimes::default()
        },
        fire_at,
        fire_at + Duration::minutes(1),
    );

    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Essay");
    assert_eq!(notes[0].expires_at, None);
    assert_eq!(notes[0].end_at, None);
    assert!(notes[0].body.starts_with("Due "));
}

#[test]
fn event_notifications_do_not_fire_after_event_end() {
    let start = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 10, 13, 0, 0).unwrap();
    let mut item = Item::new("Class").with_start(start).with_end(end);
    item.state[0].state.notification_offset_secs = Some(-2 * 60 * 60);
    let workspace = workspace_with_item(item);

    let notes = compute_due_notifications(
        &workspace,
        Utc.with_ymd_and_hms(2026, 5, 10, 13, 59, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 10, 14, 1, 0).unwrap(),
    );

    assert!(notes.is_empty());
}

#[test]
fn expired_event_notification_keys_are_available_after_event_end() {
    let start = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 10, 13, 0, 0).unwrap();
    let (workspace, scheme_id) =
        workspace_and_scheme_with_item(Item::new("Class").with_start(start).with_end(end));

    let keys = expired_event_notification_keys(
        &workspace,
        NotificationLeadTimes::default(),
        end + Duration::seconds(1),
    );

    assert_eq!(keys.len(), 1);
    assert!(keys[0].contains(&scheme_id.0.to_string()));
}

#[test]
fn expired_event_notification_keys_skip_old_events() {
    let now = Utc.with_ymd_and_hms(2026, 5, 18, 8, 0, 0).unwrap();
    let start = now - Duration::days(8);
    let end = start + Duration::hours(1);
    let workspace = workspace_with_item(Item::new("Class").with_start(start).with_end(end));

    let keys = expired_event_notification_keys(&workspace, NotificationLeadTimes::default(), now);

    assert!(keys.is_empty());
}

#[test]
fn snoozed_overdue_assignment_uses_new_fire_time() {
    let due = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let mut item = Item::new("Essay").with_end(due);
    item.state[0].state.notification_offset_secs = Some(-10 * 60);
    let workspace = workspace_with_item(item);

    let notes = compute_due_notifications(
        &workspace,
        due + Duration::minutes(9),
        due + Duration::minutes(11),
    );

    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].fire_at, due + Duration::minutes(10));
    assert!(notes[0].body.starts_with("Due "));
}

#[test]
fn completed_items_do_not_schedule_but_keys_are_available_for_cleanup() {
    let start = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let item = Item::new("done reminder").with_start(start).done();
    let item_id = item.id;
    let (workspace, scheme_id) = workspace_and_scheme_with_item(item);

    let notes = compute_due_notifications(
        &workspace,
        start - Duration::minutes(1),
        start + Duration::minutes(1),
    );
    assert!(notes.is_empty());

    let keys = notification_keys_for_item(
        &workspace,
        NotificationLeadTimes::default(),
        scheme_id,
        item_id,
        start - Duration::minutes(1),
        start + Duration::minutes(1),
    );
    assert_eq!(keys.len(), 1);
    // The OS notification key is scoped to the durable item ID so two items
    // with the same time do not collide.
    assert!(keys[0].contains(&scheme_id.0.to_string()));
    assert!(keys[0].contains(&item_id.0.to_string()));
}

#[test]
fn completed_recurring_occurrence_keys_only_target_that_instance() {
    let first = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let second = first + Duration::days(1);
    let completed = OccurrenceId::recurring_utc(first);
    let open = OccurrenceId::recurring_utc(second);
    let mut item = Item::new("standup")
        .with_start(first)
        .with_repeats(CalendarRecurrence {
            rrules: vec!["FREQ=DAILY;COUNT=2".to_string()],
            ..CalendarRecurrence::default()
        });
    item.state_for_occurrence_mut(completed.clone()).progress = -1;
    let item_id = item.id;
    let (workspace, scheme_id) = workspace_and_scheme_with_item(item);
    let from = first - Duration::minutes(1);
    let to = second + Duration::minutes(1);

    let completed_keys =
        completed_notification_keys(&workspace, NotificationLeadTimes::default(), from, to);
    let occurrence_keys = notification_keys_for_occurrence(
        &workspace,
        NotificationLeadTimes::default(),
        scheme_id,
        item_id,
        &completed,
        from,
        to,
    );
    let open_occurrence_keys = notification_keys_for_occurrence(
        &workspace,
        NotificationLeadTimes::default(),
        scheme_id,
        item_id,
        &open,
        from,
        to,
    );

    assert_eq!(completed_keys, occurrence_keys);
    assert_eq!(completed_keys.len(), 1);
    assert_eq!(open_occurrence_keys.len(), 1);
    assert_ne!(completed_keys, open_occurrence_keys);
}

#[test]
fn notification_key_is_stable_when_occurrence_is_snoozed() {
    let trigger = Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap();
    let from = trigger - Duration::hours(1);
    let to = trigger + Duration::hours(2);

    // Default schedule: a reminder fires at its start time.
    let (mut workspace, scheme_id) =
        workspace_and_scheme_with_item(Item::new("reminder").with_start(trigger));
    let before = compute_due_notifications(&workspace, from, to);
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].fire_at, trigger);

    // "Remind me later 10 minutes": a per-occurrence offset pushes the fire time
    // out on the same scheme + item, but the notification key/id must NOT change.
    // Other devices receive this offset via sync and must reuse the same id so the
    // already-delivered banner can be matched and cleared instead of stacking a
    // second one.
    workspace.schemes.get_mut(&scheme_id).unwrap().items[0].state[0]
        .state
        .notification_offset_secs = Some(-600);
    let after = compute_due_notifications(&workspace, from, to);
    assert_eq!(after.len(), 1);

    assert_eq!(
        after[0].fire_at,
        trigger + Duration::seconds(600),
        "snooze should move the fire time"
    );
    assert_eq!(
        before[0].key, after[0].key,
        "snooze must not change the notification key/id"
    );
}

#[test]
fn compute_uses_supplied_occurrence_expander() {
    let start = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let workspace = workspace_with_item(Item::new("Synthetic").with_start(start));
    let expander = SyntheticExpander { start };

    let notes = compute_due_notifications_with_expander(
        &workspace,
        NotificationLeadTimes::default(),
        start,
        start + Duration::minutes(1),
        &expander,
    );

    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Synthetic");
}

#[test]
fn duplicate_rows_collapse_to_one_notification() {
    let start = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    // Two distinct items (fresh ids) with identical text + time in one scheme —
    // the shape of a daily row that was carried/duplicated. Only one banner.
    let mut scheme = Scheme::new("Daily", 0);
    scheme
        .items
        .push(Item::new("Call dentist").with_start(start));
    scheme
        .items
        .push(Item::new("Call dentist").with_start(start));
    assert_ne!(scheme.items[0].id, scheme.items[1].id);
    let workspace = workspace_with_scheme(scheme);

    let notes = compute_due_notifications(
        &workspace,
        start - Duration::minutes(1),
        start + Duration::minutes(1),
    );

    assert_eq!(
        notes.len(),
        1,
        "identical duplicate rows schedule a single banner"
    );
    assert_eq!(notes[0].title, "Call dentist");
}

#[test]
fn distinct_reminders_at_same_time_are_not_collapsed() {
    let start = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    // Guards against an over-broad de-dupe: different text ⇒ different banner,
    // even at the same scheme/time.
    let mut scheme = Scheme::new("Daily", 0);
    scheme
        .items
        .push(Item::new("Call dentist").with_start(start));
    scheme.items.push(Item::new("Email Sam").with_start(start));
    let workspace = workspace_with_scheme(scheme);

    let notes = compute_due_notifications(
        &workspace,
        start - Duration::minutes(1),
        start + Duration::minutes(1),
    );

    assert_eq!(
        notes.len(),
        2,
        "different reminders at the same time stay distinct"
    );
}

#[test]
fn daily_queue_notification_key_is_identical_across_rollover_days() {
    let trigger = Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap();
    let from = trigger - Duration::hours(1);
    let to = trigger + Duration::hours(2);

    // "Roll over from yesterday" MOVES an item (same ItemId) from one day's
    // daily scheme into the next day's. The notification key must be identical
    // on both sides of that hop, or every device re-schedules a second banner
    // and loses snooze state after each rollover.
    let item = Item::new("pay rent").with_start(trigger);

    let key_in_daily_scheme = |date: NaiveDate| {
        let mut scheme = Scheme::new("Daily", 0);
        scheme.id = daily_queue_scheme_id(date);
        scheme.items.push(item.clone());
        let mut workspace = workspace_with_scheme(scheme);
        workspace
            .daily_queue
            .insert(date, daily_queue_scheme_id(date));
        let notes = compute_due_notifications(&workspace, from, to);
        assert_eq!(notes.len(), 1);
        notes[0].key.clone()
    };

    let day1 = NaiveDate::from_ymd_opt(2026, 7, 2).unwrap();
    let day2 = NaiveDate::from_ymd_opt(2026, 7, 3).unwrap();
    assert_eq!(
        key_in_daily_scheme(day1),
        key_in_daily_scheme(day2),
        "the key must survive the rollover hop between per-day daily schemes"
    );

    // The same item in a REGULAR scheme keys on the scheme id — moving an item
    // between ordinary schemes intentionally changes its notification identity.
    let regular = workspace_and_scheme_with_item(item.clone()).0;
    let regular_notes = compute_due_notifications(&regular, from, to);
    assert_eq!(regular_notes.len(), 1);
    assert_ne!(
        regular_notes[0].key,
        key_in_daily_scheme(day1),
        "non-daily schemes must keep scheme-scoped keys"
    );
}

fn workspace_with_item(item: Item) -> Workspace {
    workspace_and_scheme_with_item(item).0
}

fn workspace_with_scheme(scheme: Scheme) -> Workspace {
    let mut workspace = Workspace::new();
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace
}

fn workspace_and_scheme_with_item(item: Item) -> (Workspace, knotq_model::SchemeId) {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("General", 0);
    scheme.items.push(item);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    (workspace, scheme_id)
}

struct SyntheticExpander {
    start: chrono::DateTime<Utc>,
}

impl OccurrenceExpander for SyntheticExpander {
    fn expand(&self, item: &Item, _range: DateRange) -> Vec<Occurrence> {
        vec![Occurrence {
            id: OccurrenceId::Single,
            start: Some(self.start),
            end: None,
            available: None,
            kind: ItemKind::Reminder,
            occurrence_index: 0,
            state: item.single_state(),
        }]
    }

    fn next_after(&self, _item: &Item, _after: chrono::DateTime<Utc>) -> Option<Occurrence> {
        None
    }

    fn prev_before(&self, _item: &Item, _before: chrono::DateTime<Utc>) -> Option<Occurrence> {
        None
    }
}
