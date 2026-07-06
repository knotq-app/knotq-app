//! Handling user responses to delivered notifications (snooze / mark done) and
//! resolving them back to the originating occurrence.
use chrono::{DateTime, Duration, Utc};
use gpui::Context;
use knotq_commands::Command;
use knotq_model::{ItemId, ItemKind, OccurrenceId, SchemeId, Workspace};
use knotq_notifications::{
    notification_snooze_action, notification_tomorrow_morning_utc, take_notification_responses,
    NotificationResponse, ACTION_MARK_DONE, ACTION_SNOOZE_TOMORROW_MORNING,
};
use knotq_rrule::ItemOccurrenceExt;
use uuid::Uuid;

use crate::app::KnotQApp;

use super::clearing::clear_delivered_notification;
use super::common::NOTIFICATION_LOOKBACK_DAYS;

#[derive(Clone, Debug)]
pub struct NotificationActionTarget {
    pub notification_id: String,
    pub action_id: String,
    pub notification_key: Option<String>,
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub occurrence: OccurrenceId,
    pub trigger_at: DateTime<Utc>,
}

pub fn drain_notification_action_targets() -> Vec<NotificationActionTarget> {
    take_notification_responses()
        .into_iter()
        .filter_map(notification_action_target)
        .collect()
}

impl KnotQApp {
    pub(crate) fn handle_notification_action_targets(
        &mut self,
        targets: Vec<NotificationActionTarget>,
        cx: &mut Context<Self>,
    ) {
        for target in targets {
            clear_delivered_notification(&target.notification_id);
            if target.action_id == ACTION_SNOOZE_TOMORROW_MORNING {
                self.snooze_notification_target_until(
                    target,
                    notification_tomorrow_morning_utc(),
                    cx,
                );
            } else if let Some(action) = notification_snooze_action(&target.action_id) {
                self.snooze_notification_target(target, Duration::seconds(action.delay_secs), cx);
            } else if target.action_id == ACTION_MARK_DONE {
                self.mark_notification_target_done(target, cx);
            }
        }
    }

    fn snooze_notification_target(
        &mut self,
        target: NotificationActionTarget,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        self.snooze_notification_target_until(target, Utc::now() + delay, cx);
    }

    fn snooze_notification_target_until(
        &mut self,
        target: NotificationActionTarget,
        fire_at: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) {
        let Some(item_id) = self.notification_target_item_id(&target) else {
            return;
        };
        if self.notification_target_is_done(&target, item_id) {
            return;
        }
        let offset_secs = (target.trigger_at - fire_at).num_seconds();
        self.apply(
            Command::SetOccurrenceNotificationOffset {
                scheme: target.scheme_id,
                item: item_id,
                occurrence: target.occurrence,
                offset_secs: Some(offset_secs),
            },
            cx,
        );
    }

    fn mark_notification_target_done(
        &mut self,
        target: NotificationActionTarget,
        cx: &mut Context<Self>,
    ) {
        let Some(item_id) = self.notification_target_item_id(&target) else {
            return;
        };
        if self.notification_target_is_done(&target, item_id) {
            return;
        }
        self.apply(
            Command::ToggleOccurrence {
                scheme: target.scheme_id,
                item: item_id,
                occurrence: target.occurrence,
            },
            cx,
        );
    }

    fn notification_target_item_id(&self, target: &NotificationActionTarget) -> Option<ItemId> {
        resolve_notification_target_item_id(&self.workspace, target)
    }

    fn notification_target_is_done(
        &self,
        target: &NotificationActionTarget,
        item_id: ItemId,
    ) -> bool {
        self.workspace
            .scheme(target.scheme_id)
            .and_then(|scheme| scheme.item(item_id))
            .map(|item| item.state_for_occurrence(&target.occurrence).is_done())
            .unwrap_or(true)
    }
}

pub(crate) fn resolve_notification_target_item_id(
    workspace: &Workspace,
    target: &NotificationActionTarget,
) -> Option<ItemId> {
    let scheme = workspace.scheme(target.scheme_id)?;
    if scheme.item(target.item_id).is_some() {
        return Some(target.item_id);
    }

    let kind = target
        .notification_key
        .as_deref()
        .and_then(notification_key_kind);
    let scan_start = target.trigger_at - Duration::days(NOTIFICATION_LOOKBACK_DAYS);
    let scan_end = target.trigger_at + Duration::seconds(1);
    let matches = scheme
        .items
        .iter()
        .filter(|item| match kind {
            Some(kind) => notification_kind_code(item.kind()) == Some(kind),
            None => true,
        })
        .filter(|item| {
            item.occurrences(scan_start, scan_end)
                .into_iter()
                .any(|occ| {
                    occ.id == target.occurrence
                        && trigger_at_for_kind(occ.kind, occ.start, occ.end)
                            == Some(target.trigger_at)
                })
        })
        .map(|item| item.id)
        .take(2)
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [item_id] => Some(*item_id),
        _ => None,
    }
}

fn notification_action_target(response: NotificationResponse) -> Option<NotificationActionTarget> {
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
        action_id: response.action_id,
        notification_key: response.user_info.get("notification_key").cloned(),
        scheme_id,
        item_id,
        occurrence,
        trigger_at,
    })
}

fn notification_key_kind(key: &str) -> Option<&str> {
    // Keys are scheme|item|occurrence|kind.
    let parts: Vec<&str> = key.split('|').collect();
    (parts.len() == 4).then(|| parts[3])
}

fn notification_kind_code(kind: ItemKind) -> Option<&'static str> {
    match kind {
        ItemKind::Reminder => Some("r"),
        ItemKind::Event => Some("e"),
        ItemKind::Assignment => Some("a"),
        ItemKind::Procedure => None,
    }
}

fn trigger_at_for_kind(
    kind: ItemKind,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match kind {
        ItemKind::Reminder | ItemKind::Event => start,
        ItemKind::Assignment => end,
        ItemKind::Procedure => None,
    }
}
