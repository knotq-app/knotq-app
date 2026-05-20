use chrono::NaiveDate;
use knotq_commands::Command;
use knotq_model::{Item, Scheme};
use knotq_state::{daily_queue_carryover_command, daily_queue_scheme_is_blank, DailyQueueState};

#[test]
fn blank_daily_queue_scheme_is_detected() {
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
fn day_boundary_sync_updates_today_once() {
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let tomorrow = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
    let mut state = DailyQueueState::new(today, today);

    assert!(state.sync_day_boundary(tomorrow));
    assert!(!state.sync_day_boundary(tomorrow));
}
