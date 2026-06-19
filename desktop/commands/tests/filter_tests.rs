use chrono::{TimeZone, Utc};
use knotq_commands::{filter_recurring_occurrence_toggles, Command, WorkspaceCommandExt};
use knotq_model::{CalendarRecurrence, Item, OccurrenceId, Workspace};

mod support;

use support::create_root_scheme;

#[test]
fn recurring_items_only_accept_recurring_occurrence_toggles() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let recurrence = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=3".to_string()],
        ..CalendarRecurrence::default()
    };
    let item = Item::new("repeat").with_repeats(recurrence);
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    assert!(filter_recurring_occurrence_toggles(
        Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
        },
        &workspace,
    )
    .is_none());

    let recurring = OccurrenceId::recurring_utc(Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap());
    assert!(filter_recurring_occurrence_toggles(
        Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: recurring,
        },
        &workspace,
    )
    .is_some());
}

#[test]
fn batch_filter_drops_invalid_recurring_toggles() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let recurrence = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=3".to_string()],
        ..CalendarRecurrence::default()
    };
    let recurring_item = Item::new("repeat").with_repeats(recurrence);
    let recurring_item_id = recurring_item.id;
    let plain_item = Item::new("plain");
    let plain_item_id = plain_item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item: recurring_item,
        })
        .unwrap();
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 1,
            item: plain_item,
        })
        .unwrap();

    let filtered = filter_recurring_occurrence_toggles(
        Command::Batch(vec![
            Command::ToggleOccurrence {
                scheme: scheme_id,
                item: recurring_item_id,
                occurrence: OccurrenceId::Single,
            },
            Command::ToggleOccurrence {
                scheme: scheme_id,
                item: plain_item_id,
                occurrence: OccurrenceId::Single,
            },
        ]),
        &workspace,
    )
    .unwrap();

    let Command::Batch(commands) = filtered else {
        panic!("expected batch");
    };
    assert_eq!(commands.len(), 1);
}
