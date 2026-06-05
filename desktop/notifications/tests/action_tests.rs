use chrono::{Duration, TimeZone, Utc};
use knotq_commands::Command;
use knotq_model::{ItemId, OccurrenceId, SchemeId};
use knotq_notifications::{
    action_to_command_at, notification_action_target, notification_tomorrow_morning_utc_after,
    NotificationAction, NotificationActionTarget, NotificationResponse, ACTION_MARK_DONE,
    ACTION_SNOOZE_10_MINUTES, ACTION_SNOOZE_1_DAY, ACTION_SNOOZE_1_HOUR, ACTION_SNOOZE_2_HOURS,
    ACTION_SNOOZE_5_MINUTES, ACTION_SNOOZE_6_HOURS, ACTION_SNOOZE_TOMORROW_MORNING,
    NOTIFICATION_SNOOZE_ACTIONS,
};
use std::collections::BTreeMap;

#[test]
fn mark_done_action_maps_to_toggle_command() {
    let target = target(NotificationAction::MarkDone);

    let command = action_to_command_at(&target, target.trigger_at).unwrap();

    match command {
        Command::ToggleOccurrence {
            scheme,
            item,
            occurrence,
        } => {
            assert_eq!(scheme, target.scheme_id);
            assert_eq!(item, target.item_id);
            assert_eq!(occurrence, target.occurrence);
        }
        other => panic!("expected toggle command, got {other:?}"),
    }
}

#[test]
fn snooze_action_maps_to_notification_offset_command() {
    let target = target(NotificationAction::Snooze {
        delay_secs: 10 * 60,
    });
    let now = target.trigger_at - Duration::minutes(5);

    let command = action_to_command_at(&target, now).unwrap();

    match command {
        Command::SetOccurrenceNotificationOffset {
            scheme,
            item,
            occurrence,
            offset_secs,
        } => {
            assert_eq!(scheme, target.scheme_id);
            assert_eq!(item, target.item_id);
            assert_eq!(occurrence, target.occurrence);
            assert_eq!(offset_secs, Some(-5 * 60));
        }
        other => panic!("expected offset command, got {other:?}"),
    }
}

#[test]
fn notification_response_parses_expanded_snooze_action() {
    let target = target(NotificationAction::Snooze { delay_secs: 5 * 60 });
    let mut user_info = BTreeMap::new();
    user_info.insert("scheme_id".to_string(), target.scheme_id.0.to_string());
    user_info.insert("item_id".to_string(), target.item_id.0.to_string());
    user_info.insert(
        "occurrence_json".to_string(),
        serde_json::to_string(&target.occurrence).unwrap(),
    );
    user_info.insert("trigger_at".to_string(), target.trigger_at.to_rfc3339());

    let parsed = notification_action_target(NotificationResponse {
        notification_id: "note".to_string(),
        action_id: ACTION_SNOOZE_5_MINUTES.to_string(),
        user_info,
    })
    .unwrap();

    assert_eq!(
        parsed.action,
        NotificationAction::Snooze { delay_secs: 5 * 60 }
    );
}

#[test]
fn visible_snooze_actions_match_mobile_set() {
    let actions = NOTIFICATION_SNOOZE_ACTIONS
        .iter()
        .map(|action| (action.action_id, action.label, action.delay_secs))
        .collect::<Vec<_>>();

    assert_eq!(
        actions,
        vec![
            (ACTION_SNOOZE_10_MINUTES, "Snooze 10 min", 10 * 60),
            (ACTION_SNOOZE_1_HOUR, "Snooze 1 hour", 60 * 60),
            (ACTION_SNOOZE_2_HOURS, "Snooze 2 hours", 2 * 60 * 60),
            (ACTION_SNOOZE_6_HOURS, "Snooze 6 hours", 6 * 60 * 60),
            (ACTION_SNOOZE_1_DAY, "Snooze 24 hours", 24 * 60 * 60),
            (ACTION_SNOOZE_TOMORROW_MORNING, "Tomorrow Morning", 0),
        ]
    );
}

#[test]
fn tomorrow_morning_action_maps_to_local_9am_notification_offset() {
    let target = target(NotificationAction::SnoozeTomorrowMorning);
    let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
    let expected = notification_tomorrow_morning_utc_after(now);

    let command = action_to_command_at(&target, now).unwrap();

    let Command::SetOccurrenceNotificationOffset { offset_secs, .. } = command else {
        panic!("expected offset command, got {command:?}");
    };
    let fire_at = target.trigger_at - Duration::seconds(offset_secs.unwrap());
    assert_eq!(fire_at, expected);
}

#[test]
fn notification_response_parses_tomorrow_morning_action() {
    let target = target(NotificationAction::SnoozeTomorrowMorning);
    let mut user_info = BTreeMap::new();
    user_info.insert("scheme_id".to_string(), target.scheme_id.0.to_string());
    user_info.insert("item_id".to_string(), target.item_id.0.to_string());
    user_info.insert(
        "occurrence_json".to_string(),
        serde_json::to_string(&target.occurrence).unwrap(),
    );
    user_info.insert("trigger_at".to_string(), target.trigger_at.to_rfc3339());

    let parsed = notification_action_target(NotificationResponse {
        notification_id: "note".to_string(),
        action_id: ACTION_SNOOZE_TOMORROW_MORNING.to_string(),
        user_info,
    })
    .unwrap();

    assert_eq!(parsed.action, NotificationAction::SnoozeTomorrowMorning);
}

#[test]
fn notification_response_parses_action_target() {
    let target = target(NotificationAction::MarkDone);
    let mut user_info = BTreeMap::new();
    user_info.insert("scheme_id".to_string(), target.scheme_id.0.to_string());
    user_info.insert("item_id".to_string(), target.item_id.0.to_string());
    user_info.insert(
        "occurrence_json".to_string(),
        serde_json::to_string(&target.occurrence).unwrap(),
    );
    user_info.insert("trigger_at".to_string(), target.trigger_at.to_rfc3339());

    let parsed = notification_action_target(NotificationResponse {
        notification_id: "note".to_string(),
        action_id: ACTION_MARK_DONE.to_string(),
        user_info,
    })
    .unwrap();

    assert_eq!(parsed.notification_id, "note");
    assert_eq!(parsed.action, NotificationAction::MarkDone);
    assert_eq!(parsed.scheme_id, target.scheme_id);
}

fn target(action: NotificationAction) -> NotificationActionTarget {
    NotificationActionTarget {
        notification_id: "note".to_string(),
        action,
        scheme_id: SchemeId::new(),
        item_id: ItemId::new(),
        occurrence: OccurrenceId::Single,
        trigger_at: Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap(),
    }
}
