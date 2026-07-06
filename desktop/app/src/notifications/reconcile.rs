//! Reconciling the durable OS schedule with the current pending list so
//! notifications still fire after KnotQ quits.
use chrono::Utc;
use knotq_model::{Item, ItemId, SchemeId};
use knotq_notifications::{
    delivered_backlog_exceeds, DesiredPlatformSchedule, DurableNotificationSchedule,
    NotificationRequest, NotificationScheduler, PlatformSchedulePolicy, PlatformScheduleSnapshot,
    ReconciliationMode, RetentionReport, ScheduleReconciliationPlan,
    DEFAULT_DURABLE_NOTIFICATION_LIMIT,
};
use knotq_storage_json::NotificationDefaults;
use std::collections::BTreeSet;
use std::time::Duration as StdDuration;

use super::common::{
    background_schedule_policy, load_schedule_manifest, notif_log, save_schedule_manifest,
    shutdown_schedule_policy, APP_ID,
};
use super::compute::pending_notification_requests_for_item;

/// Reconcile the durable OS schedule with the current pending list so
/// notifications still fire after KnotQ quits.
pub fn schedule_os_notifications(requests: &[NotificationRequest]) -> Option<String> {
    let durable = durable_schedule_from_requests(requests);
    schedule_os_notifications_reconciled(&durable, background_schedule_policy())
}

pub fn schedule_os_notifications_for_shutdown(requests: &[NotificationRequest]) -> Option<String> {
    let durable = durable_schedule_from_requests(requests);
    schedule_os_notifications_reconciled(&durable, shutdown_schedule_policy())
}

pub(crate) fn refresh_item_os_notifications(
    scheme_id: SchemeId,
    scheme_is_daily: bool,
    item: Item,
    defaults: NotificationDefaults,
) -> Option<String> {
    let item_id = item.id;
    let requests =
        pending_notification_requests_for_item(scheme_id, scheme_is_daily, item, defaults);
    if requests.is_empty() {
        return None;
    }

    let scheduler = NotificationScheduler::new(APP_ID);
    refresh_item_os_notifications_reconciled(
        &scheduler,
        item_id,
        &requests,
        background_schedule_policy(),
    )
}

fn durable_schedule_from_requests(requests: &[NotificationRequest]) -> DurableNotificationSchedule {
    DurableNotificationSchedule::new(
        requests.iter().cloned(),
        Utc::now(),
        DEFAULT_DURABLE_NOTIFICATION_LIMIT,
    )
}

fn schedule_os_notifications_reconciled(
    durable: &DurableNotificationSchedule,
    policy: PlatformSchedulePolicy,
) -> Option<String> {
    let scheduler = NotificationScheduler::new(APP_ID);
    let snapshot = match snapshot_or_log(&scheduler) {
        Ok(snapshot) => snapshot,
        Err(msg) => return Some(msg),
    };
    let backlog_error = prune_delivered_notification_backlog(&scheduler, &snapshot, policy);
    let mut manifest = load_schedule_manifest();
    let desired = durable.platform_window(policy);
    clear_rescheduled_delivered_banners(&scheduler, &snapshot, &desired);
    let plan =
        ScheduleReconciliationPlan::new(&snapshot, &desired, &manifest, ReconciliationMode::Full);

    let cancel_error = cancel_notifications(&scheduler, &plan.to_cancel);
    let requests_to_schedule = desired.requests_for(&plan.to_schedule);
    let schedule_error = schedule_requests(&scheduler, &requests_to_schedule, policy.add_interval);
    let verify_error = verify_pending_request_ids(&scheduler, &desired, policy);
    let reconciliation_error = backlog_error.or(cancel_error).or(schedule_error);

    if reconciliation_error.is_none() {
        durable.replace_manifest(&mut manifest);
        save_schedule_manifest(&manifest);
    }

    let first_error = reconciliation_error.or(verify_error);

    if let Some(err) = first_error.as_ref() {
        notif_log(&format!(
            "notification reconciliation left a partial OS schedule: {err}"
        ));
        return first_error;
    }

    notif_log(&format!(
        "OS schedule reconciled: {} kept, {} added/updated, {} canceled, {} desired",
        plan.kept_count,
        plan.to_schedule.len(),
        plan.to_cancel.len(),
        plan.desired_count
    ));

    first_error
}

fn prune_delivered_notification_backlog(
    scheduler: &NotificationScheduler,
    snapshot: &PlatformScheduleSnapshot,
    policy: PlatformSchedulePolicy,
) -> Option<String> {
    if !delivered_backlog_exceeds(snapshot, policy.max_delivered_backlog) {
        return None;
    }

    match scheduler.remove_all_delivered() {
        Ok(()) => {
            notif_log(&format!(
                "OS cleared {} delivered notification(s) to stay below the platform request cap",
                snapshot.delivered().len()
            ));
            None
        }
        Err(err) => {
            let msg = format!("{err}");
            notif_log(&format!(
                "clear delivered OS notification backlog failed: {msg}"
            ));
            Some(msg)
        }
    }
}

fn refresh_item_os_notifications_reconciled(
    scheduler: &NotificationScheduler,
    item_id: ItemId,
    requests: &[NotificationRequest],
    policy: PlatformSchedulePolicy,
) -> Option<String> {
    let snapshot = match snapshot_or_log(scheduler) {
        Ok(snapshot) => snapshot,
        Err(msg) => return Some(msg),
    };
    let desired = DurableNotificationSchedule::new(
        requests.iter().cloned(),
        Utc::now(),
        DEFAULT_DURABLE_NOTIFICATION_LIMIT,
    )
    .platform_window(policy);
    clear_rescheduled_delivered_banners(scheduler, &snapshot, &desired);
    let mut manifest = load_schedule_manifest();
    let plan = ScheduleReconciliationPlan::new(
        &snapshot,
        &desired,
        &manifest,
        ReconciliationMode::Targeted,
    );

    if plan.to_cancel.is_empty() && plan.to_schedule.is_empty() {
        notif_log(&format!(
            "OS item {} notification schedule unchanged, skipping",
            item_id,
        ));
        return None;
    }

    let cancel_error = cancel_notifications(scheduler, &plan.to_cancel);
    let requests_to_schedule = desired.requests_for(&plan.to_schedule);
    let schedule_error = schedule_requests(scheduler, &requests_to_schedule, policy.add_interval);
    let reconciliation_error = cancel_error.or(schedule_error);

    if reconciliation_error.is_none() {
        manifest.update_requests(requests);
        save_schedule_manifest(&manifest);
    }

    notif_log(&format!(
        "OS refreshed item {} notification schedule: {} kept, {} added/updated, {} canceled, {} desired",
        item_id,
        plan.kept_count,
        plan.to_schedule.len(),
        plan.to_cancel.len(),
        plan.desired_count
    ));

    reconciliation_error
}

/// Snapshot the platform schedule, logging (and surfacing) the failure message
/// on error so reconciliation callers can bail out uniformly.
fn snapshot_or_log(scheduler: &NotificationScheduler) -> Result<PlatformScheduleSnapshot, String> {
    platform_schedule_snapshot(scheduler).map_err(|msg| {
        notif_log(&format!("platform OS notification snapshot failed: {msg}"));
        msg
    })
}

fn platform_schedule_snapshot(
    scheduler: &NotificationScheduler,
) -> Result<PlatformScheduleSnapshot, String> {
    let pending = scheduler.pending_ids().map_err(|err| format!("{err}"))?;
    let delivered = scheduler.delivered_ids().map_err(|err| format!("{err}"))?;
    Ok(PlatformScheduleSnapshot::new(pending, delivered))
}

/// Remove delivered banners whose id is also in the *future* desired set.
///
/// Notification ids are stable per occurrence (the fire time is not part of the
/// key), so a delivered banner sharing an id with a future desired request can
/// only mean that occurrence was rescheduled — e.g. "remind me later" picked on
/// another device and synced here. The already-fired banner would otherwise
/// linger next to the rescheduled one. A normally-delivered, still-open
/// notification has already fired, so its id never appears in `desired`; and a
/// later recurrence carries a different occurrence id, so it is never matched.
fn clear_rescheduled_delivered_banners(
    scheduler: &NotificationScheduler,
    snapshot: &PlatformScheduleSnapshot,
    desired: &DesiredPlatformSchedule,
) {
    let stale = desired
        .ids()
        .intersection(snapshot.delivered())
        .cloned()
        .collect::<Vec<_>>();
    if stale.is_empty() {
        return;
    }
    match scheduler.remove_delivered(&stale) {
        Ok(()) => notif_log(&format!(
            "OS cleared {} delivered banner(s) for rescheduled occurrence(s)",
            stale.len()
        )),
        Err(err) => notif_log(&format!(
            "remove_delivered for {} rescheduled banner(s) failed: {err}",
            stale.len()
        )),
    }
}

fn cancel_notifications(
    scheduler: &NotificationScheduler,
    ids: &BTreeSet<String>,
) -> Option<String> {
    if ids.is_empty() {
        return None;
    }

    let ids = ids.iter().cloned().collect::<Vec<_>>();
    let cancel_result = scheduler.cancel(&ids);
    // Also remove delivered notifications so stale banners don't linger in
    // the notification center after an event is deleted or rescheduled.
    if let Err(err) = scheduler.remove_delivered(&ids) {
        notif_log(&format!(
            "remove_delivered for {} stale notification(s) failed: {err}",
            ids.len()
        ));
    }
    match cancel_result {
        Ok(()) => {
            notif_log(&format!(
                "OS canceled {} stale/changed pending notification(s)",
                ids.len()
            ));
            None
        }
        Err(err) => {
            let msg = format!("{err}");
            notif_log(&format!("cancel stale OS notifications failed: {msg}"));
            Some(msg)
        }
    }
}

fn schedule_requests(
    scheduler: &NotificationScheduler,
    requests: &[NotificationRequest],
    add_interval: StdDuration,
) -> Option<String> {
    let eligible: Vec<&NotificationRequest> =
        requests.iter().filter(|r| r.fire_at > Utc::now()).collect();
    if eligible.is_empty() {
        notif_log("OS schedule add pass: 0/0 request(s)");
        return None;
    }

    let results = scheduler.schedule_batch(&eligible, add_interval);
    let mut first_error = None;
    let mut scheduled = 0;
    for (request, result) in eligible.iter().zip(results) {
        match result {
            Ok(()) => {
                scheduled += 1;
                let delta = (request.fire_at - Utc::now()).num_seconds();
                notif_log(&format!(
                    "OS scheduled \"{}\" id={} in {}s",
                    request.title, request.id, delta
                ));
            }
            Err(err) => {
                let msg = format!("{err}");
                notif_log(&format!(
                    "OS schedule failed for \"{}\" id={}: {msg}",
                    request.title, request.id
                ));
                if first_error.is_none() {
                    first_error = Some(msg);
                }
            }
        }
    }
    notif_log(&format!(
        "OS schedule add pass: {scheduled}/{} request(s)",
        eligible.len()
    ));
    first_error
}

fn verify_pending_request_ids(
    scheduler: &NotificationScheduler,
    desired: &knotq_notifications::DesiredPlatformSchedule,
    policy: PlatformSchedulePolicy,
) -> Option<String> {
    if desired.ids().is_empty() {
        return None;
    }

    if !policy.initial_verify_delay.is_zero() {
        std::thread::sleep(policy.initial_verify_delay);
    }

    let snapshot = match platform_schedule_snapshot(scheduler) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            let msg = err.to_string();
            notif_log(&format!(
                "pending OS notification verification failed: {msg}"
            ));
            return Some(msg);
        }
    };
    let report = RetentionReport::new(&snapshot, desired);

    if report.is_complete() {
        notif_log(&format!(
            "OS retained {}/{} pending notification(s)",
            report.retained_count, report.desired_count
        ));
        return None;
    }

    // Only retry if we lost a significant fraction (more than half missing).
    if report.missing.len() > report.desired_count / 2 {
        if !policy.retry_verify_delay.is_zero() {
            std::thread::sleep(policy.retry_verify_delay);
        }
        let retry_snapshot = match platform_schedule_snapshot(scheduler) {
            Ok(s) => s,
            Err(_) => {
                // Log but don't fail on retry.
                notif_log("OS notification verification retry snapshot failed");
                return None;
            }
        };
        let retry_report = RetentionReport::new(&retry_snapshot, desired);
        if retry_report.is_complete() {
            notif_log(&format!(
                "OS retained {}/{} pending notification(s) after retry",
                retry_report.retained_count, retry_report.desired_count
            ));
            return None;
        }

        let msg = format!(
            "OS did not retain {}/{} scheduled notification(s); missing: {preview}",
            retry_report.missing.len(),
            retry_report.desired_count,
            preview = retry_report.missing_preview(3)
        );
        notif_log(&msg);
        if retry_report.retained_count == 0 && retry_report.desired_count > 0 {
            return Some(msg);
        }
    } else {
        notif_log(&format!(
            "OS retained {}/{} pending notification(s) ({} missing, within tolerance)",
            report.retained_count,
            report.desired_count,
            report.missing.len()
        ));
    }

    None
}
