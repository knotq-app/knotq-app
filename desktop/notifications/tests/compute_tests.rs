use chrono::{Duration, TimeZone, Utc};
use knotq_date_util::DateRange;
use knotq_model::{Item, ItemKind, NodeRef, Occurrence, OccurrenceId, Scheme, Workspace};
use knotq_notifications::{
    compute_due_notifications, compute_due_notifications_with_expander,
    compute_due_notifications_with_lead_times, expired_event_notification_keys,
    notification_keys_for_item, NotificationKind, NotificationLeadTimes,
};
use knotq_rrule::OccurrenceExpander;

#[test]
fn event_notifications_fire_at_start_time() {
    let start = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 10, 13, 0, 0).unwrap();
    let workspace = workspace_with_item(Item::new("Class").with_start(start).with_end(end));

    let notes = compute_due_notifications(
        &workspace,
        Utc.with_ymd_and_hms(2026, 5, 10, 11, 59, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 10, 12, 1, 0).unwrap(),
    );

    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].kind, NotificationKind::Event);
    assert_eq!(notes[0].fire_at, start);
    assert_eq!(notes[0].expires_at, Some(end));
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

fn workspace_with_item(item: Item) -> Workspace {
    workspace_and_scheme_with_item(item).0
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
