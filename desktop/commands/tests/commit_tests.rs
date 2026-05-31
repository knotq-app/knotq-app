use chrono::{DateTime, TimeZone, Utc};
use knotq_commands::{
    event_popup_commit_commands, event_popup_delete_command, Command, DateEditScope, DateKind,
    EventDeleteScope, EventPopupDraft,
};
use knotq_model::{CalendarDateTime, CalendarRecurrence, Item, OccurrenceId};

fn dt(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, day, hour, 0, 0).unwrap()
}

#[test]
fn deleting_one_recurring_occurrence_adds_exdate() {
    let scheme_id = knotq_model::SchemeId::new();
    let item_id = knotq_model::ItemId::new();
    let original_start = CalendarDateTime::utc(Utc.with_ymd_and_hms(2026, 1, 7, 10, 0, 0).unwrap());
    let mut item = Item::new("standup")
        .with_start(dt(5, 10))
        .with_end(dt(5, 11));
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=5".to_string()],
        ..CalendarRecurrence::default()
    });

    let command = event_popup_delete_command(
        &item,
        scheme_id,
        item_id,
        OccurrenceId::Recurring {
            original_start: original_start.clone(),
        },
        2,
        EventDeleteScope::ThisEvent,
    )
    .expect("delete command");

    match command {
        Command::SetItemRecurrence { repeats, .. } => {
            let repeats = repeats.expect("remaining recurrence");
            assert_eq!(repeats.exdates, vec![original_start]);
            assert_eq!(repeats.rrules, vec!["FREQ=DAILY;COUNT=5"]);
        }
        other => panic!("expected recurrence edit, got {other:?}"),
    }
}

#[test]
fn deleting_this_and_future_truncates_simple_recurrence() {
    let scheme_id = knotq_model::SchemeId::new();
    let item_id = knotq_model::ItemId::new();
    let original_start = CalendarDateTime::utc(Utc.with_ymd_and_hms(2026, 1, 7, 10, 0, 0).unwrap());
    let mut item = Item::new("standup")
        .with_start(dt(5, 10))
        .with_end(dt(5, 11));
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=5".to_string()],
        ..CalendarRecurrence::default()
    });

    let command = event_popup_delete_command(
        &item,
        scheme_id,
        item_id,
        OccurrenceId::Recurring {
            original_start: original_start.clone(),
        },
        2,
        EventDeleteScope::AllFuture,
    )
    .expect("delete command");

    match command {
        Command::SetItemRecurrence { repeats, .. } => {
            let repeats = repeats.expect("truncated recurrence");
            assert_eq!(
                repeats.rrules,
                vec!["FREQ=DAILY;INTERVAL=1;UNTIL=20260107T095959Z"]
            );
        }
        other => panic!("expected recurrence truncation, got {other:?}"),
    }
}

#[test]
fn clearing_recurrence_from_later_occurrence_keeps_selected_occurrence_dates() {
    let recurrence = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY;COUNT=5".to_string()],
        ..CalendarRecurrence::default()
    };
    let mut item = Item::new("standup")
        .with_start(dt(5, 10))
        .with_end(dt(5, 11));
    item.repeats = Some(recurrence);

    let draft = EventPopupDraft {
        scheme_id: knotq_model::SchemeId::new(),
        item_id: knotq_model::ItemId::new(),
        occurrence: OccurrenceId::recurring_utc(dt(7, 10)),
        occurrence_index: 2,
        draft_start: Some(dt(7, 14)),
        draft_end: Some(dt(7, 15)),
        draft_repeats: None,
        draft_notification_offset_secs: None,
        draft_done: false,
        start_dirty: false,
        end_dirty: false,
        repeats_dirty: true,
        notification_dirty: false,
        done_dirty: false,
    };

    let commands = event_popup_commit_commands(&item, &draft, DateEditScope::AllEvents);

    assert_eq!(commands.len(), 3);
    match &commands[0] {
        Command::SetItemDate {
            kind: DateKind::Start,
            date,
            ..
        } => assert_eq!(*date, Some(dt(7, 14))),
        other => panic!("expected promoted start date, got {other:?}"),
    }
    match &commands[1] {
        Command::SetItemDate {
            kind: DateKind::End,
            date,
            ..
        } => assert_eq!(*date, Some(dt(7, 15))),
        other => panic!("expected promoted end date, got {other:?}"),
    }
    match &commands[2] {
        Command::SetItemRecurrence { repeats, .. } => assert!(repeats.is_none()),
        other => panic!("expected recurrence clear, got {other:?}"),
    }
}
