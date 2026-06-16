use chrono::NaiveDate;
use knotq_commands::Command;
use knotq_model::{daily_queue_scheme_id, Item, Scheme, Workspace};
use knotq_state::{
    daily_queue_carryover_command, daily_queue_scheme_is_blank, last_nonempty_daily_queue_day,
    DailyQueueState,
};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

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

    let command = daily_queue_carryover_command(previous.id, &previous, today.id, &today);

    assert!(matches!(command, Some(Command::Batch(_))));
}

#[test]
fn carryover_inserts_into_empty_today() {
    let mut previous = Scheme::new("Yesterday", 0);
    previous.items.push(Item::new("Finish draft"));
    let today = Scheme::new("Today", 0);

    let command = daily_queue_carryover_command(previous.id, &previous, today.id, &today);

    let Some(Command::Batch(commands)) = command else {
        panic!("expected carryover batch");
    };
    assert!(matches!(
        commands.as_slice(),
        [Command::InsertItem { position: 0, .. }]
    ));
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
