use chrono::{DateTime, TimeZone, Utc};
use knotq_commands::{
    event_popup_commit_commands, event_popup_delete_command, recurrence_can_delete_future,
    reset_after_trigger_notification_to_default_command, Command, DateEditScope, DateKind,
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

/// A recurrence the editor cannot express as a simple rule has no "delete this
/// and all future" — there is nothing to put an UNTIL on.
#[test]
fn only_a_simple_recurrence_can_be_truncated_at_an_occurrence() {
    let simple = CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;BYDAY=MO".into()],
        ..Default::default()
    };
    assert!(recurrence_can_delete_future(&simple));

    let with_rdates = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY".into()],
        rdates: vec![CalendarDateTime::utc(dt(9, 10))],
        ..Default::default()
    };
    assert!(
        !recurrence_can_delete_future(&with_rdates),
        "explicit extra dates are not expressible as a simple rule"
    );

    let two_rules = CalendarRecurrence {
        rrules: vec!["FREQ=DAILY".into(), "FREQ=WEEKLY".into()],
        ..Default::default()
    };
    assert!(!recurrence_can_delete_future(&two_rules));
}

/// Deleting "this and all future" from the very first occurrence deletes the
/// item outright rather than leaving an empty series behind.
#[test]
fn deleting_from_the_first_occurrence_deletes_the_whole_item() {
    let scheme_id = knotq_model::SchemeId::new();
    let item_id = knotq_model::ItemId::new();
    let mut item = Item::new("standup");
    item.marker = knotq_model::ItemMarker::Checkbox;
    item.start = Some(dt(5, 9));
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=DAILY".into()],
        ..Default::default()
    });
    let occurrence = OccurrenceId::Recurring {
        original_start: CalendarDateTime::utc(dt(5, 9)),
    };

    let command = event_popup_delete_command(
        &item,
        scheme_id,
        item_id,
        occurrence,
        0,
        EventDeleteScope::AllFuture,
    );

    assert!(
        matches!(command, Some(Command::DeleteItem { item, .. }) if item == item_id),
        "truncating at index 0 leaves no occurrences, so the item goes"
    );
}

/// A later occurrence truncates the series with an UNTIL just before it.
#[test]
fn deleting_from_a_later_occurrence_truncates_the_series() {
    let scheme_id = knotq_model::SchemeId::new();
    let item_id = knotq_model::ItemId::new();
    let mut item = Item::new("standup");
    item.marker = knotq_model::ItemMarker::Checkbox;
    item.start = Some(dt(5, 9));
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=DAILY".into()],
        ..Default::default()
    });
    let occurrence = OccurrenceId::Recurring {
        original_start: CalendarDateTime::utc(dt(8, 9)),
    };

    let command = event_popup_delete_command(
        &item,
        scheme_id,
        item_id,
        occurrence,
        3,
        EventDeleteScope::AllFuture,
    );

    let Some(Command::SetItemRecurrence {
        repeats: Some(repeats),
        ..
    }) = command
    else {
        panic!("expected the series to be truncated, got {command:?}");
    };
    assert!(
        repeats.rrules[0].contains("UNTIL="),
        "truncation is expressed as an UNTIL: {:?}",
        repeats.rrules[0]
    );
}

/// Deleting every event ignores the recurrence entirely.
#[test]
fn deleting_all_events_removes_the_item_whatever_the_recurrence() {
    let scheme_id = knotq_model::SchemeId::new();
    let item_id = knotq_model::ItemId::new();
    let mut item = Item::new("standup");
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=DAILY".into()],
        ..Default::default()
    });

    let command = event_popup_delete_command(
        &item,
        scheme_id,
        item_id,
        OccurrenceId::Single,
        0,
        EventDeleteScope::AllEvents,
    );

    assert!(matches!(command, Some(Command::DeleteItem { .. })));
}

/// A snooze that moved a notification *earlier* is discarded when the event is
/// rescheduled to a future time — otherwise the old negative offset would fire
/// the reminder before the new start.
#[test]
fn rescheduling_into_the_future_clears_a_negative_notification_offset() {
    let scheme_id = knotq_model::SchemeId::new();
    let item_id = knotq_model::ItemId::new();
    let mut item = Item::new("review");
    item.marker = knotq_model::ItemMarker::Checkbox;
    item.state_for_occurrence_mut(OccurrenceId::Single)
        .notification_offset_secs = Some(-600);

    let command = reset_after_trigger_notification_to_default_command(
        &item,
        scheme_id,
        item_id,
        OccurrenceId::Single,
        Some(future_dt(5, 9)),
        None,
        dt(5, 9),
    );
    assert!(
        matches!(
            command,
            Some(Command::SetOccurrenceNotificationOffset {
                offset_secs: None,
                ..
            })
        ),
        "the stale early offset is cleared, got {command:?}"
    );

    // A positive offset is a deliberate "remind me after", not a snooze, and a
    // trigger already in the past has nothing left to reset.
    item.state_for_occurrence_mut(OccurrenceId::Single)
        .notification_offset_secs = Some(600);
    assert!(reset_after_trigger_notification_to_default_command(
        &item,
        scheme_id,
        item_id,
        OccurrenceId::Single,
        Some(future_dt(5, 9)),
        None,
        dt(5, 9),
    )
    .is_none());
}
