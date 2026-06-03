use chrono::{DateTime, TimeZone, Utc};
use knotq_commands::{
    event_popup_commit_commands, event_popup_delete_command, Command, DateEditScope, DateKind,
    EventDeleteScope, EventPopupDraft,
};
use knotq_model::{CalendarDateTime, CalendarRecurrence, Item, OccurrenceId};

fn dt(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, day, hour, 0, 0).unwrap()
}

fn future_dt(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2035, 1, day, hour, 0, 0).unwrap()
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

#[test]
fn moving_future_event_clears_after_trigger_notification_override() {
    let scheme_id = knotq_model::SchemeId::new();
    let item_id = knotq_model::ItemId::new();
    let mut item = Item::new("meeting")
        .with_start(future_dt(5, 10))
        .with_end(future_dt(5, 11));
    item.id = item_id;
    item.state[0].state.notification_offset_secs = Some(-30 * 60);

    let draft = EventPopupDraft {
        scheme_id,
        item_id,
        occurrence: OccurrenceId::Single,
        occurrence_index: 0,
        draft_start: Some(future_dt(6, 10)),
        draft_end: Some(future_dt(6, 11)),
        draft_repeats: None,
        draft_notification_offset_secs: Some(-30 * 60),
        draft_done: false,
        start_dirty: true,
        end_dirty: true,
        repeats_dirty: false,
        notification_dirty: false,
        done_dirty: false,
    };

    let commands = event_popup_commit_commands(&item, &draft, DateEditScope::AllEvents);

    assert!(commands.iter().any(|command| matches!(
        command,
        Command::SetOccurrenceNotificationOffset {
            scheme,
            item,
            occurrence: OccurrenceId::Single,
            offset_secs: None,
        } if *scheme == scheme_id && *item == item_id
    )));
}

#[test]
fn moving_future_event_preserves_before_trigger_notification_override() {
    let scheme_id = knotq_model::SchemeId::new();
    let item_id = knotq_model::ItemId::new();
    let mut item = Item::new("meeting")
        .with_start(future_dt(5, 10))
        .with_end(future_dt(5, 11));
    item.id = item_id;
    item.state[0].state.notification_offset_secs = Some(30 * 60);

    let draft = EventPopupDraft {
        scheme_id,
        item_id,
        occurrence: OccurrenceId::Single,
        occurrence_index: 0,
        draft_start: Some(future_dt(6, 10)),
        draft_end: Some(future_dt(6, 11)),
        draft_repeats: None,
        draft_notification_offset_secs: Some(30 * 60),
        draft_done: false,
        start_dirty: true,
        end_dirty: true,
        repeats_dirty: false,
        notification_dirty: false,
        done_dirty: false,
    };

    let commands = event_popup_commit_commands(&item, &draft, DateEditScope::AllEvents);

    assert!(!commands
        .iter()
        .any(|command| matches!(command, Command::SetOccurrenceNotificationOffset { .. })));
}
