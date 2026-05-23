use chrono::{DateTime, Duration, Utc};
use gpui::Context;
use knotq_commands::Command;
use knotq_model::{Item, ItemId, ItemKind, OccurrenceId, Scheme, SchemeId, Workspace};
use knotq_notifications::{
    compute_due_notifications_with_lead_times, delivered_backlog_exceeds, delivered_cleanup_ids,
    expired_event_notification_keys, notification_keys_for_item, DurableNotificationSchedule,
    NotificationLeadTimes, PlatformSchedulePolicy, PlatformScheduleSnapshot, ReconciliationMode,
    RetentionReport, ScheduleManifest, ScheduleReconciliationPlan, ScheduledNotification,
    DEFAULT_DURABLE_NOTIFICATION_LIMIT,
};
use knotq_notifications::{
    take_notification_responses, AuthorizationStatus, NotificationRequest, NotificationResponse,
    NotificationScheduler, PlatformStatus, ACTION_MARK_DONE, ACTION_SNOOZE_10_MINUTES,
    ACTION_SNOOZE_1_HOUR,
};
use knotq_rrule::ItemOccurrenceExt;
use knotq_storage_json::data_dir;
use knotq_storage_json::NotificationDefaults;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;
use uuid::Uuid;

use crate::app::KnotQApp;

/// Keep notification diagnostics in the app data directory so sandboxed builds
/// do not need temporary-directory write access outside the app container.
pub fn notif_log(msg: &str) {
    use std::io::Write;
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("knotq-notif.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "[{}] {}", Utc::now().format("%H:%M:%S"), msg);
    }
}

#[cfg(target_os = "macos")]
const PLATFORM_OS_PENDING_LIMIT: usize = 16;
#[cfg(not(target_os = "macos"))]
const PLATFORM_OS_PENDING_LIMIT: usize = DEFAULT_DURABLE_NOTIFICATION_LIMIT;
const SCHEDULE_HORIZON_DAYS: i64 = 14;
const PLATFORM_OS_HARD_HORIZON: StdDuration = StdDuration::from_secs(32 * 24 * 60 * 60);
pub(crate) const APP_ID: &str = "com.enigmadux.knotq";
const CATEGORY_ID: &str = "knotq-reminder";
const SCHEDULE_MANIFEST_FILE: &str = "notification_schedule_manifest.json";
const NOTIFICATION_LOOKBACK_DAYS: i64 = 7;

static AUTHORIZATION_REQUESTED: AtomicBool = AtomicBool::new(false);

fn base_schedule_policy() -> PlatformSchedulePolicy {
    PlatformSchedulePolicy::new(PLATFORM_OS_PENDING_LIMIT)
        .with_max_schedule_horizon(PLATFORM_OS_HARD_HORIZON)
}

fn background_schedule_policy() -> PlatformSchedulePolicy {
    let policy = base_schedule_policy();
    #[cfg(target_os = "macos")]
    {
        return policy
            .with_add_interval(StdDuration::from_millis(150))
            .with_verify_delays(StdDuration::from_millis(500), StdDuration::from_millis(750));
    }
    #[cfg(not(target_os = "macos"))]
    {
        policy
    }
}

fn shutdown_schedule_policy() -> PlatformSchedulePolicy {
    base_schedule_policy()
}

/// Request notification authorization without blocking. Call this from the
/// main thread (e.g. during app construction) so macOS can show the system
/// permission dialog. The actual scheduling happens later in the notification
/// service and will succeed once the user grants permission.
pub fn request_authorization_nonblocking() {
    if AUTHORIZATION_REQUESTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let scheduler = NotificationScheduler::new(APP_ID);
        notif_log("requesting authorization (nonblocking, main thread)");
        scheduler.request_authorization_nonblocking();
    }
}

pub fn configure_notification_handling() {
    NotificationScheduler::new(APP_ID).configure_notification_handling();
}

pub fn notification_availability_error() -> Option<String> {
    let scheduler = NotificationScheduler::new(APP_ID);
    let status = scheduler.platform_status();
    if !matches!(status, PlatformStatus::Available) {
        return Some(platform_status_message(status));
    }

    match scheduler.authorization_status() {
        Ok(status) if status.can_deliver() => None,
        Ok(AuthorizationStatus::NotDetermined) => None,
        Ok(status) if status.unavailable_reason().is_some() => {
            status.unavailable_reason().map(str::to_string)
        }
        Ok(_) => Some("notification authorization status is unknown".to_string()),
        Err(err) => Some(format!("{err}")),
    }
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

pub(crate) fn pending_notification_requests_for_item(
    scheme_id: SchemeId,
    item: Item,
    defaults: NotificationDefaults,
) -> Vec<NotificationRequest> {
    let mut workspace = Workspace::empty();
    let mut scheme = Scheme::new("", 0);
    scheme.id = scheme_id;
    scheme.items.push(item);
    workspace.schemes.insert(scheme_id, scheme);

    pending_notifications(&workspace, defaults)
        .into_iter()
        .map(notification_request)
        .collect()
}

pub(crate) fn refresh_item_os_notifications(
    scheme_id: SchemeId,
    item: Item,
    defaults: NotificationDefaults,
) -> Option<String> {
    let item_id = item.id;
    let requests = pending_notification_requests_for_item(scheme_id, item, defaults);
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

fn durable_schedule_from_requests(requests: &[NotificationRequest]) -> DurableNotificationSchedule {
    DurableNotificationSchedule::new(
        requests.iter().cloned(),
        Utc::now(),
        DEFAULT_DURABLE_NOTIFICATION_LIMIT,
    )
}

pub fn clear_expired_event_notifications(
    workspace: &Workspace,
    defaults: NotificationDefaults,
    now: DateTime<Utc>,
) -> Option<String> {
    let mut ids = expired_event_notification_keys(workspace, lead_times(defaults), now)
        .into_iter()
        .map(|key| os_notification_id(&key))
        .collect::<Vec<_>>();
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

fn schedule_os_notifications_reconciled(
    durable: &DurableNotificationSchedule,
    policy: PlatformSchedulePolicy,
) -> Option<String> {
    let scheduler = NotificationScheduler::new(APP_ID);
    let snapshot = match platform_schedule_snapshot(&scheduler) {
        Ok(snapshot) => snapshot,
        Err(msg) => {
            notif_log(&format!("platform OS notification snapshot failed: {msg}"));
            return Some(msg);
        }
    };
    let backlog_error = prune_delivered_notification_backlog(&scheduler, &snapshot, policy);
    let mut manifest = load_schedule_manifest();
    let desired = durable.platform_window(policy);
    let plan =
        ScheduleReconciliationPlan::new(&snapshot, &desired, &manifest, ReconciliationMode::Full);

    let legacy_cancel_error = cancel_notifications(&scheduler, snapshot.pending_legacy());
    let cancel_error = cancel_notifications(&scheduler, &plan.to_cancel);
    let requests_to_schedule = desired.requests_for(&plan.to_schedule);
    let schedule_error = schedule_requests(&scheduler, &requests_to_schedule, policy.add_interval);
    let verify_error = verify_pending_request_ids(&scheduler, &desired, policy);
    let reconciliation_error = backlog_error
        .or(legacy_cancel_error)
        .or(cancel_error)
        .or(schedule_error);

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
    let snapshot = match platform_schedule_snapshot(scheduler) {
        Ok(snapshot) => snapshot,
        Err(msg) => {
            notif_log(&format!("platform OS notification snapshot failed: {msg}"));
            return Some(msg);
        }
    };
    let desired = DurableNotificationSchedule::new(
        requests.iter().cloned(),
        Utc::now(),
        DEFAULT_DURABLE_NOTIFICATION_LIMIT,
    )
    .platform_window(policy);
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

fn platform_schedule_snapshot(
    scheduler: &NotificationScheduler,
) -> Result<PlatformScheduleSnapshot, String> {
    let pending = scheduler.pending_ids().map_err(|err| format!("{err}"))?;
    let delivered = scheduler.delivered_ids().map_err(|err| format!("{err}"))?;
    Ok(PlatformScheduleSnapshot::new(pending, delivered))
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
            let msg = format!("{err}");
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

fn load_schedule_manifest() -> ScheduleManifest {
    let path = schedule_manifest_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ScheduleManifest::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_schedule_manifest(manifest: &ScheduleManifest) {
    let path = schedule_manifest_path();
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            notif_log(&format!(
                "failed to create notification manifest directory: {err}"
            ));
            return;
        }
    }

    match serde_json::to_vec_pretty(&manifest) {
        Ok(raw) => {
            if let Err(err) = std::fs::write(&path, raw) {
                notif_log(&format!("failed to write notification manifest: {err}"));
            }
        }
        Err(err) => {
            notif_log(&format!("failed to serialize notification manifest: {err}"));
        }
    }
}

fn schedule_manifest_path() -> std::path::PathBuf {
    data_dir().join(SCHEDULE_MANIFEST_FILE)
}

#[derive(Clone, Debug)]
pub struct NotificationUpdate {
    pub requests: Vec<NotificationRequest>,
}

pub fn clear_item_notifications(
    workspace: &Workspace,
    defaults: NotificationDefaults,
    scheme_id: SchemeId,
    item_id: ItemId,
) {
    let now = Utc::now();
    let ids = notification_keys_for_item(
        workspace,
        lead_times(defaults),
        scheme_id,
        item_id,
        now - Duration::days(NOTIFICATION_LOOKBACK_DAYS),
        now + Duration::days(SCHEDULE_HORIZON_DAYS),
    )
    .into_iter()
    .map(|key| os_notification_id(&key))
    .collect::<Vec<_>>();
    if ids.is_empty() {
        return;
    }
    let scheduler = NotificationScheduler::new(APP_ID);
    if let Err(err) = scheduler.cancel(&ids) {
        eprintln!("failed to cancel item notifications: {err}");
    }
    if let Err(err) = scheduler.remove_delivered(&ids) {
        eprintln!("failed to clear delivered item notifications: {err}");
    }
}

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
            match target.action_id.as_str() {
                ACTION_SNOOZE_10_MINUTES => {
                    self.snooze_notification_target(target, Duration::minutes(10), cx);
                }
                ACTION_SNOOZE_1_HOUR => {
                    self.snooze_notification_target(target, Duration::hours(1), cx);
                }
                ACTION_MARK_DONE => {
                    self.mark_notification_target_done(target, cx);
                }
                _ => {}
            }
        }
    }

    fn snooze_notification_target(
        &mut self,
        target: NotificationActionTarget,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let Some(item_id) = self.notification_target_item_id(&target) else {
            return;
        };
        if self.notification_target_is_done(&target, item_id) {
            return;
        }
        let fire_at = Utc::now() + delay;
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

fn resolve_notification_target_item_id(
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
    request
}

fn os_notification_id(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    format!(
        "knotq-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    )
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
    // New keys are scheme|item|occurrence|kind|fire_at. Legacy keys are
    // scheme|occurrence|kind|fire_at.
    match key.split('|').count() {
        5 => key.split('|').nth(3),
        4 => key.split('|').nth(2),
        _ => None,
    }
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

fn clear_delivered_notification(id: &str) {
    if id.is_empty() {
        return;
    }
    let scheduler = NotificationScheduler::new(APP_ID);
    if let Err(err) = scheduler.remove_delivered(&[id.to_string()]) {
        eprintln!("failed to clear delivered notification {id}: {err}");
    }
}

fn platform_status_message(status: PlatformStatus) -> String {
    match status {
        PlatformStatus::Available => "notifications are available".to_string(),
        PlatformStatus::Unavailable(reason) => {
            format!("notifications unavailable: {reason}")
        }
        PlatformStatus::Unsupported(reason) => {
            format!("notifications unsupported: {reason}")
        }
    }
}

fn lead_times(defaults: NotificationDefaults) -> NotificationLeadTimes {
    NotificationLeadTimes {
        reminder_offset_secs: 0,
        event_offset_secs: defaults.event_offset_secs,
        assignment_offset_secs: defaults.assignment_offset_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn notification_request_has_stable_key() {
        let now = Utc::now();
        let note1 = NotificationRequest::new("stable-key", now, "T", "B");
        let note2 = NotificationRequest::new("stable-key", now, "T", "B");
        assert_eq!(note1.id, note2.id);
    }

    #[test]
    fn schedule_horizon_is_two_weeks() {
        assert_eq!(SCHEDULE_HORIZON_DAYS, 14);
    }

    #[test]
    fn notification_request_carries_expiration_metadata() {
        let fire_at = Utc.with_ymd_and_hms(2026, 5, 21, 12, 0, 0).unwrap();
        let expires_at = fire_at + Duration::hours(1);
        let note = ScheduledNotification {
            key: "key".to_string(),
            fire_at,
            expires_at: Some(expires_at),
            title: "Class".to_string(),
            body: "From Thu, 12:00 PM to 1:00 PM".to_string(),
            kind: knotq_notifications::NotificationKind::Event,
            trigger_at: fire_at,
            scheme_id: SchemeId::new(),
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
        };

        let request = notification_request(note);

        assert_eq!(request.expires_at, Some(expires_at));
        let expected_expires_at = expires_at.to_rfc3339();
        assert_eq!(
            request.user_info.get("expires_at").map(String::as_str),
            Some(expected_expires_at.as_str())
        );
    }

    #[test]
    fn notification_target_resolves_stale_item_id_from_unique_occurrence() {
        let trigger_at = Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap();
        let item = Item::new("meeting").with_start(trigger_at);
        let item_id = item.id;
        let mut scheme = Scheme::new("Work", 0);
        let scheme_id = scheme.id;
        scheme.items.push(item);
        let mut workspace = Workspace::new();
        workspace.schemes.insert(scheme_id, scheme);

        let target = NotificationActionTarget {
            notification_id: "notification".to_string(),
            action_id: ACTION_MARK_DONE.to_string(),
            notification_key: Some(format!("{scheme_id}|single|r|{}", trigger_at.to_rfc3339())),
            scheme_id,
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
            trigger_at,
        };

        assert_eq!(
            resolve_notification_target_item_id(&workspace, &target),
            Some(item_id)
        );
    }

    #[test]
    fn notification_target_does_not_guess_when_stale_item_id_is_ambiguous() {
        let trigger_at = Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap();
        let mut scheme = Scheme::new("Work", 0);
        let scheme_id = scheme.id;
        scheme.items.push(Item::new("first").with_start(trigger_at));
        scheme
            .items
            .push(Item::new("second").with_start(trigger_at));
        let mut workspace = Workspace::new();
        workspace.schemes.insert(scheme_id, scheme);

        let target = NotificationActionTarget {
            notification_id: "notification".to_string(),
            action_id: ACTION_MARK_DONE.to_string(),
            notification_key: Some(format!("{scheme_id}|single|r|{}", trigger_at.to_rfc3339())),
            scheme_id,
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
            trigger_at,
        };

        assert_eq!(
            resolve_notification_target_item_id(&workspace, &target),
            None
        );
    }
}
