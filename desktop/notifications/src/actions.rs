use chrono::{DateTime, Duration, Utc};
use knotq_commands::Command;
use knotq_model::{ItemId, OccurrenceId, SchemeId};
use uuid::Uuid;

use crate::platform_provider::{
    notification_snooze_action, NotificationResponse, ACTION_MARK_DONE,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationAction {
    Snooze { delay_secs: i64 },
    MarkDone,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationActionTarget {
    pub notification_id: String,
    pub action: NotificationAction,
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub occurrence: OccurrenceId,
    pub trigger_at: DateTime<Utc>,
}

pub fn action_to_command(target: &NotificationActionTarget) -> Option<Command> {
    action_to_command_at(target, Utc::now())
}

pub fn action_to_command_at(
    target: &NotificationActionTarget,
    now: DateTime<Utc>,
) -> Option<Command> {
    match target.action {
        NotificationAction::MarkDone => Some(Command::ToggleOccurrence {
            scheme: target.scheme_id,
            item: target.item_id,
            occurrence: target.occurrence.clone(),
        }),
        NotificationAction::Snooze { delay_secs } => {
            snooze_command(target, now, Duration::seconds(delay_secs))
        }
    }
}

pub fn drain_notification_action_targets() -> Vec<NotificationActionTarget> {
    crate::take_notification_responses()
        .into_iter()
        .filter_map(notification_action_target)
        .collect()
}

pub fn notification_action_target(
    response: NotificationResponse,
) -> Option<NotificationActionTarget> {
    let action = notification_action(&response.action_id)?;
    let scheme_id = SchemeId(Uuid::parse_str(response.user_info.get("scheme_id")?).ok()?);
    let item_id = ItemId(Uuid::parse_str(response.user_info.get("item_id")?).ok()?);
    let occurrence = response
        .user_info
        .get("occurrence_json")
        .and_then(|raw| serde_json::from_str(raw).ok())?;
    let trigger_at = DateTime::parse_from_rfc3339(response.user_info.get("trigger_at")?)
        .ok()?
        .with_timezone(&Utc);
    Some(NotificationActionTarget {
        notification_id: response.notification_id,
        action,
        scheme_id,
        item_id,
        occurrence,
        trigger_at,
    })
}

pub fn notification_action(action_id: &str) -> Option<NotificationAction> {
    if let Some(action) = notification_snooze_action(action_id) {
        return Some(NotificationAction::Snooze {
            delay_secs: action.delay_secs,
        });
    }
    match action_id {
        ACTION_MARK_DONE => Some(NotificationAction::MarkDone),
        _ => None,
    }
}

fn snooze_command(
    target: &NotificationActionTarget,
    now: DateTime<Utc>,
    delay: Duration,
) -> Option<Command> {
    let fire_at = now + delay;
    Some(Command::SetOccurrenceNotificationOffset {
        scheme: target.scheme_id,
        item: target.item_id,
        occurrence: target.occurrence.clone(),
        offset_secs: Some((target.trigger_at - fire_at).num_seconds()),
    })
}
