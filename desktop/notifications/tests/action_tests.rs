use chrono::{Duration, TimeZone, Utc};
use knotq_commands::Command;
use knotq_model::{ItemId, OccurrenceId, SchemeId};
use knotq_notifications::{
    action_to_command_at, notification_action_target, NotificationAction, NotificationActionTarget,
    NotificationResponse, ACTION_MARK_DONE,
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
    let target = target(NotificationAction::SnoozeShort);
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
