//! Removing scheduled and delivered OS notifications when items expire, are
//! completed, deleted, or rescheduled.
use chrono::{DateTime, Duration, Utc};
use knotq_model::{Item, ItemId, OccurrenceId, SchemeId, Workspace};
use knotq_notifications::{
    completed_notification_keys, delivered_cleanup_ids, expired_event_notification_keys,
    notification_keys_for_item, notification_keys_for_occurrence, NotificationScheduler,
};
use knotq_storage_json::NotificationDefaults;
use std::collections::BTreeSet;

use super::common::{
    keys_to_ids, lead_times, load_schedule_manifest, notif_log, save_schedule_manifest,
    workspace_for_item, APP_ID, NOTIFICATION_LOOKBACK_DAYS, SCHEDULE_HORIZON_DAYS,
};

pub fn clear_expired_event_notifications(
    workspace: &Workspace,
    defaults: NotificationDefaults,
    now: DateTime<Utc>,
) -> Option<String> {
    let mut ids = keys_to_ids(expired_event_notification_keys(
        workspace,
        lead_times(defaults),
        now,
    ));
    let mut manifest = load_schedule_manifest();
    let expired_manifest_ids = manifest.prune_expired(now);
    if !expired_manifest_ids.is_empty() {
        save_schedule_manifest(&manifest);
        ids.extend(expired_manifest_ids);
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return None;
    }

    let scheduler = NotificationScheduler::new(APP_ID);
    let delivered = match scheduler.delivered_ids() {
        Ok(ids) => ids.into_iter().collect::<BTreeSet<_>>(),
        Err(err) => {
            let msg = format!("{err}");
            notif_log(&format!(
                "expired delivered OS notification lookup failed: {msg}"
            ));
            return Some(msg);
        }
    };
    ids = delivered_cleanup_ids(ids, &delivered);
    if ids.is_empty() {
        return None;
    }

    match scheduler.remove_delivered(&ids) {
        Ok(()) => {
            notif_log(&format!(
                "OS requested removal for {} expired delivered event notification(s)",
                ids.len()
            ));
            None
        }
        Err(err) => {
            let msg = format!("{err}");
            notif_log(&format!(
                "remove expired delivered event notifications failed: {msg}"
            ));
            Some(msg)
        }
    }
}

pub fn clear_completed_notifications(
    workspace: &Workspace,
    defaults: NotificationDefaults,
    now: DateTime<Utc>,
) -> Option<String> {
    let ids = keys_to_ids(completed_notification_keys(
        workspace,
        lead_times(defaults),
        now - Duration::days(NOTIFICATION_LOOKBACK_DAYS),
        now + Duration::days(SCHEDULE_HORIZON_DAYS),
    ));
    clear_os_notification_ids(ids, "completed occurrence")
}

pub fn clear_item_notifications(
    workspace: &Workspace,
    defaults: NotificationDefaults,
    scheme_id: SchemeId,
    item_id: ItemId,
) {
    let now = Utc::now();
    let ids = keys_to_ids(notification_keys_for_item(
        workspace,
        lead_times(defaults),
        scheme_id,
        item_id,
        now - Duration::days(NOTIFICATION_LOOKBACK_DAYS),
        now + Duration::days(SCHEDULE_HORIZON_DAYS),
    ));
    let _ = clear_os_notification_ids(ids, "item");
}

pub(crate) fn clear_item_notifications_for_item(
    scheme_id: SchemeId,
    item: Item,
    defaults: NotificationDefaults,
) {
    let item_id = item.id;
    let workspace = workspace_for_item(scheme_id, item);
    clear_item_notifications(&workspace, defaults, scheme_id, item_id);
}

pub(crate) fn clear_occurrence_notifications_for_item(
    scheme_id: SchemeId,
    item: Item,
    occurrence: OccurrenceId,
    defaults: NotificationDefaults,
) {
    let item_id = item.id;
    let workspace = workspace_for_item(scheme_id, item);

    let now = Utc::now();
    let ids = keys_to_ids(notification_keys_for_occurrence(
        &workspace,
        lead_times(defaults),
        scheme_id,
        item_id,
        &occurrence,
        now - Duration::days(NOTIFICATION_LOOKBACK_DAYS),
        now + Duration::days(SCHEDULE_HORIZON_DAYS),
    ));
    let _ = clear_os_notification_ids(ids, "completed occurrence");
}

fn clear_os_notification_ids(mut ids: Vec<String>, context: &str) -> Option<String> {
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return None;
    }

    let scheduler = NotificationScheduler::new(APP_ID);
    let cancel_error = match scheduler.cancel(&ids) {
        Ok(()) => None,
        Err(err) => {
            let msg = format!("{err}");
            notif_log(&format!(
                "failed to cancel {context} notification(s): {msg}"
            ));
            Some(msg)
        }
    };
    if let Err(err) = scheduler.remove_delivered(&ids) {
        notif_log(&format!(
            "failed to clear delivered {context} notification(s): {err}"
        ));
        if cancel_error.is_none() {
            return Some(format!("{err}"));
        }
    }
    if cancel_error.is_none() {
        notif_log(&format!(
            "OS cleared {} {context} notification(s)",
            ids.len()
        ));
    }
    cancel_error
}

pub(crate) fn clear_delivered_notification(id: &str) {
    if id.is_empty() {
        return;
    }
    let scheduler = NotificationScheduler::new(APP_ID);
    if let Err(err) = scheduler.remove_delivered(&[id.to_string()]) {
        eprintln!("failed to clear delivered notification {id}: {err}");
    }
}
