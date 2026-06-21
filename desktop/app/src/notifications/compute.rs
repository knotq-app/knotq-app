//! Computing the pending notification list and converting scheduled
//! notifications into platform requests.
use chrono::{DateTime, Duration, Utc};
use knotq_model::{Item, SchemeId, Workspace};
use knotq_notifications::{
    compute_due_notifications_with_lead_times, NotificationRequest, ScheduledNotification,
    DEFAULT_DURABLE_NOTIFICATION_LIMIT,
};
use knotq_storage_json::NotificationDefaults;
use knotq_sync::NotificationScheduleSnapshot;
use sha2::{Digest, Sha256};

use super::common::{
    lead_times, os_notification_id, workspace_for_item, CATEGORY_ID, SCHEDULE_HORIZON_DAYS,
};

#[derive(Clone, Debug)]
pub struct NotificationUpdate {
    pub requests: Vec<NotificationRequest>,
}

pub fn pending_notifications(
    workspace: &Workspace,
    defaults: NotificationDefaults,
) -> Vec<ScheduledNotification> {
    let now = Utc::now();
    compute_due_notifications_with_lead_times(
        workspace,
        lead_times(defaults),
        now,
        now + Duration::days(SCHEDULE_HORIZON_DAYS),
    )
    .into_iter()
    .filter(|note| note.fire_at > now)
    .take(DEFAULT_DURABLE_NOTIFICATION_LIMIT)
    .collect()
}

pub(crate) fn notification_schedule_snapshot(
    workspace: &Workspace,
    defaults: NotificationDefaults,
    now: DateTime<Utc>,
    sequence: u64,
) -> NotificationScheduleSnapshot {
    let window_start = DateTime::from_naive_utc_and_offset(
        now.date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always valid"),
        Utc,
    );
    let window_end = window_start + Duration::days(SCHEDULE_HORIZON_DAYS);
    let mut notifications = compute_due_notifications_with_lead_times(
        workspace,
        lead_times(defaults),
        window_start,
        window_end,
    );
    notifications.sort_by(|left, right| {
        left.fire_at
            .cmp(&right.fire_at)
            .then_with(|| left.key.cmp(&right.key))
    });

    let mut hasher = Sha256::new();
    hasher.update(b"knotq.notification_schedule.v1");
    hasher.update([0]);
    hasher.update(window_start.to_rfc3339().as_bytes());
    hasher.update([0]);
    hasher.update(window_end.to_rfc3339().as_bytes());
    for notification in &notifications {
        hasher.update([0]);
        let json = serde_json::to_vec(notification).unwrap_or_default();
        hasher.update(json);
    }
    let digest = hasher.finalize();
    let hash = digest.iter().map(|byte| format!("{byte:02x}")).collect();

    NotificationScheduleSnapshot {
        sequence,
        hash,
        window_start,
        window_end,
        occurrence_count: notifications.len(),
    }
}

pub(crate) fn pending_notification_requests_for_item(
    scheme_id: SchemeId,
    item: Item,
    defaults: NotificationDefaults,
) -> Vec<NotificationRequest> {
    let workspace = workspace_for_item(scheme_id, item);

    pending_notifications(&workspace, defaults)
        .into_iter()
        .map(notification_request)
        .collect()
}

/// Recompute the pending notification list used by the durable OS schedule.
pub fn recompute_pending(
    workspace: &Workspace,
    defaults: NotificationDefaults,
) -> NotificationUpdate {
    let pending = pending_notifications(workspace, defaults);
    let requests: Vec<NotificationRequest> =
        pending.into_iter().map(notification_request).collect();

    NotificationUpdate { requests }
}

pub(crate) fn notification_request(note: ScheduledNotification) -> NotificationRequest {
    let occurrence_json = serde_json::to_string(&note.occurrence).unwrap_or_default();
    let key = note.key;
    let request_id = os_notification_id(&key);
    let mut request =
        NotificationRequest::new(request_id.clone(), note.fire_at, note.title, note.body)
            .expires_at(note.expires_at)
            .group(request_id)
            .user_info("notification_key", key)
            .category(CATEGORY_ID)
            .user_info("scheme_id", note.scheme_id.0.to_string())
            .user_info("item_id", note.item_id.0.to_string())
            .user_info("occurrence_json", occurrence_json)
            .user_info("trigger_at", note.trigger_at.to_rfc3339());
    if let Some(expires_at) = note.expires_at {
        request = request.user_info("expires_at", expires_at.to_rfc3339());
    }
    if let Some(end_at) = note.end_at {
        request = request.user_info("end_at", end_at.to_rfc3339());
    }
    request
}
