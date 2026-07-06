use chrono::{Duration, Local, NaiveDate, TimeZone, Timelike, Utc};
use knotq_commands::Command;
use knotq_model::{
    daily_queue_displaced_item_id, daily_queue_scheme_id, DocumentId, Item, NodeRef, Scheme,
    SchemeId, Workspace,
};
use knotq_notifications::{
    compute_due_notifications, compute_due_notifications_with_lead_times, NotificationKind,
    NotificationLeadTimes,
};
use knotq_state::{
    daily_queue_carryover_command, daily_queue_scheme_is_blank, last_nonempty_daily_queue_day,
    make_default_workspace_for_date, DailyQueueState,
};

mod support;

use support::{date, test_state};

/// Insert a daily-queue scheme for `day` carrying `items` (text only).
fn insert_daily(workspace: &mut Workspace, day: NaiveDate, items: &[&str]) {
    let id = daily_queue_scheme_id(day);
    let mut scheme = Scheme::new(format!("Daily {day}"), 0);
    scheme.id = id;
    scheme.items = items.iter().map(|text| Item::new(*text)).collect();
    workspace.daily_queue.insert(day, id);
    workspace.schemes.insert(id, scheme);
}

#[test]
fn blank_daily_queue_scheme_is_detected() {
    let scheme = Scheme::new("Daily", 0);
    assert!(daily_queue_scheme_is_blank(&scheme));

    let mut scheme = Scheme::new("Daily", 0);
    scheme.items.push(Item::new(""));
    assert!(daily_queue_scheme_is_blank(&scheme));
}

#[test]
fn carryover_moves_incomplete_items_to_today() {
    let mut previous = Scheme::new("Yesterday", 0);
    previous.items.push(Item::new("Finish draft"));
    let mut today = Scheme::new("Today", 0);
    today.items.push(Item::new(""));

    let command =
        daily_queue_carryover_command(previous.id, date(2026, 6, 15), &previous, today.id, &today);

    assert!(matches!(command, Some(Command::Batch(_))));
}

#[test]
fn carryover_inserts_into_empty_today() {
    let mut previous = Scheme::new("Yesterday", 0);
    previous.items.push(Item::new("Finish draft"));
    let today = Scheme::new("Today", 0);

    let command =
        daily_queue_carryover_command(previous.id, date(2026, 6, 15), &previous, today.id, &today);

    let Some(Command::Batch(commands)) = command else {
        panic!("expected carryover batch");
    };
    // The source row is re-identified in place on yesterday (delete + insert of
    // the displaced copy), then the carried row — keeping the SOURCE id — is
    // inserted into today.
    let source_id = previous.items[0].id;
    match commands.as_slice() {
        [Command::DeleteItem { item, .. }, Command::InsertItem {
            position: 0,
            item: displaced,
            ..
        }, Command::InsertItem {
            position: 0,
            item: carried,
            ..
        }] => {
            assert_eq!(*item, source_id);
            assert_eq!(
                displaced.id,
                daily_queue_displaced_item_id(source_id, date(2026, 6, 15))
            );
            assert_eq!(carried.id, source_id, "carried row keeps the source id");
        }
        other => panic!("unexpected carryover batch shape: {other:?}"),
    }
}

/// Re-running carryover after it already carried (today holds the source ids)
/// must be a no-op — the id-based skip is what makes a double click or a
/// sync-race re-roll idempotent.
#[test]
fn carryover_is_idempotent_once_ids_are_in_today() {
    let mut previous = Scheme::new("Yesterday", 0);
    previous.items.push(Item::new("Finish draft"));
    let mut today = Scheme::new("Today", 0);
    today.items.push(previous.items[0].clone());

    let command =
        daily_queue_carryover_command(previous.id, date(2026, 6, 15), &previous, today.id, &today);
    assert!(command.is_none(), "repeat carryover must be a no-op");
}

#[test]
fn last_nonempty_day_prefers_yesterday() {
    let today = date(2026, 6, 16);
    let mut workspace = Workspace::new();
    insert_daily(&mut workspace, date(2026, 6, 15), &["Finish draft"]);
    insert_daily(&mut workspace, date(2026, 6, 10), &["Older item"]);

    assert_eq!(
        last_nonempty_daily_queue_day(&workspace, today),
        Some(date(2026, 6, 15))
    );
}

#[test]
fn last_nonempty_day_skips_blank_days_within_window() {
    let today = date(2026, 6, 16);
    let mut workspace = Workspace::new();
    // Yesterday exists but is blank (single empty placeholder row).
    insert_daily(&mut workspace, date(2026, 6, 15), &[""]);
    insert_daily(&mut workspace, date(2026, 6, 9), &["Plan the week"]);

    assert_eq!(
        last_nonempty_daily_queue_day(&workspace, today),
        Some(date(2026, 6, 9))
    );
}

#[test]
fn last_nonempty_day_ignores_content_older_than_two_weeks() {
    let today = date(2026, 6, 16);
    let mut workspace = Workspace::new();
    // 15 days back is just outside the two-week lookback (offsets 1..=14).
    insert_daily(&mut workspace, date(2026, 6, 1), &["Too old to roll over"]);

    assert_eq!(last_nonempty_daily_queue_day(&workspace, today), None);
}

#[test]
fn day_boundary_sync_updates_today_once() {
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let tomorrow = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
    let mut state = DailyQueueState::new(today, today);

    assert!(state.sync_day_boundary(tomorrow));
    assert!(!state.sync_day_boundary(tomorrow));
}

#[test]
fn default_workspace_uses_fixed_starter_ids_and_plain_calendar_titles() {
    let workspace = make_default_workspace_for_date(date(2026, 6, 18));
    let root = workspace.folder(workspace.root).unwrap();
    let starter_ids = [
        "00000000-0000-8000-8000-000000000101"
            .parse::<SchemeId>()
            .unwrap(),
        "00000000-0000-8000-8000-000000000102"
            .parse::<SchemeId>()
            .unwrap(),
        "00000000-0000-8000-8000-000000000103"
            .parse::<SchemeId>()
            .unwrap(),
    ];
    let starter_documents = [
        "00000000-0000-8000-8000-000000000201"
            .parse::<DocumentId>()
            .unwrap(),
        "00000000-0000-8000-8000-000000000202"
            .parse::<DocumentId>()
            .unwrap(),
        "00000000-0000-8000-8000-000000000203"
            .parse::<DocumentId>()
            .unwrap(),
    ];

    for (scheme_id, document_id) in starter_ids.into_iter().zip(starter_documents) {
        assert!(root.children.contains(&NodeRef::Scheme(scheme_id)));
        assert_eq!(workspace.scheme_sync[&scheme_id].id, document_id);
    }

    let mut highlight_count = 0;
    let mut saw_work_session = false;
    for scheme in workspace.iter_schemes() {
        for item in &scheme.items {
            let text = item.text();
            if text == "Work session" {
                let end = item.end.unwrap().with_timezone(&Local);
                assert_eq!((end.hour(), end.minute()), (15, 0));
                saw_work_session = true;
            }
            assert!(
                !text.contains("~~"),
                "starter workspace should not include strikethrough examples: {text}"
            );
            // No recurring reminders in the starter workspace: a repeat would
            // schedule a notification for every new user, forever.
            assert!(
                item.repeats.is_none(),
                "starter workspace should not seed a recurring reminder (notification spam): {text}"
            );
            if text.contains("==") {
                highlight_count += 1;
            }
            if text.trim_start().starts_with('#') {
                assert!(text.trim_start().starts_with("### "));
            }
            if item.start.is_some() || item.end.is_some() || item.repeats.is_some() {
                assert!(text.len() <= 18, "calendar title is too long: {text}");
                assert!(
                    !["#", "**", "*", "==", "~~"]
                        .iter()
                        .any(|marker| text.contains(marker)),
                    "calendar title should be plain text: {text}"
                );
            }
        }
    }
    assert_eq!(highlight_count, 2);
    assert!(saw_work_session);

    // Exactly one seeded dated item is incomplete, and it's in the past — the
    // overdue example. Nothing future is incomplete, so a brand-new user's
    // starter workspace schedules no notifications.
    let seed_date = date(2026, 6, 18);
    let incomplete_dated: Vec<(String, NaiveDate)> = workspace
        .iter_schemes()
        .flat_map(|scheme| scheme.items.iter())
        .filter(|item| item.start.is_some() || item.end.is_some())
        .filter(|item| !item.state.iter().all(|s| s.state.is_done()))
        .map(|item| {
            let when = item
                .end
                .or(item.start)
                .unwrap()
                .with_timezone(&Local)
                .date_naive();
            (item.text().to_string(), when)
        })
        .collect();
    assert_eq!(
        incomplete_dated.len(),
        1,
        "starter workspace should have exactly one incomplete dated item (the overdue example): {incomplete_dated:?}"
    );
    assert!(
        incomplete_dated[0].1 < seed_date,
        "the one incomplete dated item should be overdue (past): {incomplete_dated:?}"
    );
}

/// Seed a daily-queue scheme for `day` from full `Item`s (dates, markers) and
/// register it so notification compute and carryover both see it.
fn seed_daily_scheme(
    state: &mut knotq_state::AppState,
    day: NaiveDate,
    items: Vec<Item>,
) -> SchemeId {
    let id = daily_queue_scheme_id(day);
    let mut scheme = Scheme::new(format!("Daily {day}"), 0);
    scheme.id = id;
    scheme.items = items;
    state.workspace.daily_queue.insert(day, id);
    state.workspace.schemes.insert(id, scheme);
    state.mark_scheme_dirty(id);
    id
}

/// Carryover must hand the dated annotation to today's carried row so the
/// reminder keeps tracking the live item: after rolling over, the notification
/// fires at the same time on TODAY's daily scheme and the carried item — with no
/// orphaned duplicate left on yesterday. The carried row keeps the SOURCE item
/// id and daily schemes share the stable "daily" key fragment, so the OS-level
/// notification key is IDENTICAL across the rollover: the pending schedule,
/// delivered banner, and snooze state all survive on every device.
#[test]
fn carryover_moves_reminder_notification_to_today_target() {
    let yesterday = date(2026, 6, 18);
    let today = date(2026, 6, 19);
    // Start-only ⇒ Reminder; future-dated so it schedules.
    let reminder_at = Utc.with_ymd_and_hms(2026, 6, 20, 15, 0, 0).unwrap();

    let mut state = test_state();
    let reminder_item = Item::new("Call dentist").with_start(reminder_at);
    let source_item_id = reminder_item.id;
    let yesterday_id = seed_daily_scheme(&mut state, yesterday, vec![reminder_item]);
    // Today is just its blank placeholder row.
    let today_id = seed_daily_scheme(&mut state, today, vec![Item::new("")]);

    // Default reminder lead time is 0s, so fire_at == start; window straddles it.
    let from = reminder_at - Duration::minutes(1);
    let to = reminder_at + Duration::minutes(1);

    // Before: the reminder is scheduled on YESTERDAY's daily scheme.
    let before = compute_due_notifications(&state.workspace, from, to);
    assert_eq!(before.len(), 1, "exactly one reminder before carryover");
    assert_eq!(before[0].kind, NotificationKind::Reminder);
    assert_eq!(before[0].scheme_id, yesterday_id);
    assert_eq!(before[0].item_id, source_item_id);
    let fire_at = before[0].fire_at;
    let key_before = before[0].key.clone();

    // Roll yesterday forward into today.
    let command = {
        let prev = state.workspace.scheme(yesterday_id).unwrap();
        let today_scheme = state.workspace.scheme(today_id).unwrap();
        daily_queue_carryover_command(yesterday_id, yesterday, prev, today_id, today_scheme)
            .expect("carryover should produce a command")
    };
    state.apply_command(command);

    // The carried row keeps the SOURCE id, and the dated annotation moved with
    // it; yesterday's leftover is re-identified with the displaced id, stripped.
    let (carried_id, carried_start) = {
        let today_scheme = state.workspace.scheme(today_id).unwrap();
        let carried = today_scheme
            .items
            .iter()
            .find(|item| item.text() == "Call dentist")
            .expect("carried item in today");
        (carried.id, carried.start)
    };
    assert_eq!(
        carried_id, source_item_id,
        "carried row keeps the source id"
    );
    assert_eq!(carried_start, Some(reminder_at));
    let displaced = state
        .workspace
        .scheme(yesterday_id)
        .unwrap()
        .item(daily_queue_displaced_item_id(source_item_id, yesterday))
        .expect("displaced archive row on yesterday");
    assert!(
        displaced.start.is_none(),
        "archived row's date is stripped on carryover"
    );

    // After: still exactly one reminder, now tracking TODAY's carried item — and
    // because the item id carried and daily schemes share the "daily" key
    // fragment, it is the SAME notification identity (pending schedule, delivered
    // banner, and snooze state survive).
    let after = compute_due_notifications(&state.workspace, from, to);
    assert_eq!(
        after.len(),
        1,
        "exactly one reminder after carryover (no orphan left on yesterday, no duplicate)"
    );
    assert_eq!(after[0].kind, NotificationKind::Reminder);
    assert_eq!(
        after[0].scheme_id, today_id,
        "notification tracks to today's daily scheme"
    );
    assert_eq!(
        after[0].item_id, carried_id,
        "notification tracks to the carried item"
    );
    assert_eq!(
        after[0].fire_at, fire_at,
        "fire time is unchanged across the rollover"
    );
    assert_eq!(
        after[0].key, key_before,
        "notification key is identical across the rollover"
    );
}

/// The doubling failure mode (a carried row that landed twice — an optimistic
/// insert that vanished mid-sync and got re-rolled, or two devices rolling the
/// same row forward) leaves today holding two identical dated rows with distinct
/// ids. The schedule must still surface exactly ONE banner, so the user never
/// gets a duplicate notification even when the row itself duplicated.
#[test]
fn duplicated_carried_rows_schedule_a_single_notification() {
    let today = date(2026, 6, 19);
    let reminder_at = Utc.with_ymd_and_hms(2026, 6, 20, 9, 0, 0).unwrap();

    let mut state = test_state();
    let today_id = seed_daily_scheme(
        &mut state,
        today,
        vec![
            Item::new("Submit report").with_start(reminder_at),
            Item::new("Submit report").with_start(reminder_at),
        ],
    );

    let notes = compute_due_notifications(
        &state.workspace,
        reminder_at - Duration::minutes(1),
        reminder_at + Duration::minutes(1),
    );
    assert_eq!(
        notes.len(),
        1,
        "duplicated carried rows must collapse to one notification"
    );
    assert_eq!(notes[0].scheme_id, today_id);
    assert_eq!(notes[0].title, "Submit report");
}

/// Same tracking guarantee for an event (start+end): the whole calendar
/// annotation moves to the carried row, so the single event notification follows
/// it to today and still fires at the start.
#[test]
fn carryover_moves_event_notification_to_today_target() {
    let yesterday = date(2026, 6, 18);
    let today = date(2026, 6, 19);
    let start = Utc.with_ymd_and_hms(2026, 6, 21, 9, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 6, 21, 10, 0, 0).unwrap();

    let mut state = test_state();
    let event_item = Item::new("Team standup").with_start(start).with_end(end);
    let yesterday_id = seed_daily_scheme(&mut state, yesterday, vec![event_item]);
    let today_id = seed_daily_scheme(&mut state, today, vec![Item::new("")]);

    // event_offset 0 ⇒ fire_at == start; window straddles start (before end).
    let lead = NotificationLeadTimes {
        event_offset_secs: 0,
        ..NotificationLeadTimes::default()
    };
    let from = start - Duration::minutes(1);
    let to = start + Duration::minutes(1);

    let before = compute_due_notifications_with_lead_times(&state.workspace, lead, from, to);
    assert_eq!(before.len(), 1, "one event notification before carryover");
    assert_eq!(before[0].kind, NotificationKind::Event);
    assert_eq!(before[0].scheme_id, yesterday_id);
    let fire_at = before[0].fire_at;
    let key_before = before[0].key.clone();

    let command = {
        let prev = state.workspace.scheme(yesterday_id).unwrap();
        let today_scheme = state.workspace.scheme(today_id).unwrap();
        daily_queue_carryover_command(yesterday_id, yesterday, prev, today_id, today_scheme)
            .expect("carryover should produce a command")
    };
    state.apply_command(command);

    let carried_id = {
        let today_scheme = state.workspace.scheme(today_id).unwrap();
        let carried = today_scheme
            .items
            .iter()
            .find(|item| item.text() == "Team standup")
            .expect("carried event in today");
        assert_eq!(
            carried.start,
            Some(start),
            "event start moved to carried row"
        );
        assert_eq!(carried.end, Some(end), "event end moved to carried row");
        carried.id
    };

    let after = compute_due_notifications_with_lead_times(&state.workspace, lead, from, to);
    assert_eq!(after.len(), 1, "one event notification after carryover");
    assert_eq!(after[0].kind, NotificationKind::Event);
    assert_eq!(after[0].scheme_id, today_id);
    assert_eq!(after[0].item_id, carried_id);
    assert_eq!(after[0].fire_at, fire_at);
    assert_eq!(
        after[0].key, key_before,
        "event notification key is identical across the rollover"
    );
}
