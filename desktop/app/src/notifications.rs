use chrono::{DateTime, Duration, Utc};
use gpui::Context;
use knotq_commands::Command;
use knotq_model::{Item, ItemId, ItemKind, OccurrenceId, Scheme, SchemeId, Workspace};
use knotq_notifications::{
    compute_due_notifications_with_lead_times, expired_event_notification_keys,
    notification_keys_for_item, NotificationLeadTimes, ScheduledNotification,
};
use knotq_notifications::{
    take_notification_responses, AuthorizationStatus, NotificationRequest, NotificationResponse,
    NotificationScheduler, PlatformStatus, ACTION_MARK_DONE, ACTION_SNOOZE_10_MINUTES,
    ACTION_SNOOZE_1_HOUR,
};
use knotq_rrule::ItemOccurrenceExt;
#[cfg(target_os = "macos")]
use knotq_storage_json::data_dir;
use knotq_storage_json::NotificationDefaults;
#[cfg(target_os = "macos")]
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::time::Duration as StdDuration;
use uuid::Uuid;

use crate::app::KnotQApp;

/// Log to /tmp/knotq-notif.log so we can diagnose issues even when GPUI
/// swallows stderr.
pub fn notif_log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/knotq-notif.log")
    {
        let _ = writeln!(f, "[{}] {}", Utc::now().format("%H:%M:%S"), msg);
    }
}

const MAX_PENDING_NOTIFICATIONS: usize = 64;
const SCHEDULE_HORIZON_DAYS: i64 = 14;
pub(crate) const APP_ID: &str = "com.enigmadux.knotq";
const CATEGORY_ID: &str = "knotq-reminder";
#[cfg(target_os = "macos")]
const SCHEDULE_MANIFEST_FILE: &str = "notification_schedule_manifest.json";

static AUTHORIZATION_REQUESTED: AtomicBool = AtomicBool::new(false);

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
    .take(MAX_PENDING_NOTIFICATIONS)
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
    let ids = requests
        .iter()
        .map(|request| request.id.clone())
        .collect::<Vec<_>>();
    let cancel_error = scheduler.cancel(&ids).err().map(|err| format!("{err}"));
    // Also remove delivered versions so stale banners are cleaned up.
    let _ = scheduler.remove_delivered(&ids);
    #[cfg(target_os = "macos")]
    if cancel_error.is_none() {
        std::thread::sleep(StdDuration::from_millis(100));
    }
    let schedule_error = schedule_requests(&scheduler, &requests);

    #[cfg(target_os = "macos")]
    save_item_schedule_manifest_entries(&requests);

    notif_log(&format!(
        "OS refreshed {} pending notification request(s) for item {}",
        requests.len(),
        item_id
    ));
    cancel_error.or(schedule_error)
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
    #[cfg(target_os = "macos")]
    {
        return schedule_os_notifications_reconciled(requests);
    }

    #[cfg(not(target_os = "macos"))]
    {
        return schedule_os_notifications_replace_all(requests);
    }
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
    #[cfg(target_os = "macos")]
    ids.extend(prune_expired_schedule_manifest(now));
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return None;
    }

    let scheduler = NotificationScheduler::new(APP_ID);
    match scheduler.remove_delivered(&ids) {
        Ok(()) => {
            notif_log(&format!(
                "OS removed {} expired delivered event notification(s)",
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

#[cfg(target_os = "macos")]
fn schedule_os_notifications_reconciled(requests: &[NotificationRequest]) -> Option<String> {
    let scheduler = NotificationScheduler::new(APP_ID);
    let pending = match managed_pending_ids(&scheduler) {
        Ok(ids) => ids,
        Err(msg) => {
            notif_log(&format!("pending OS notification lookup failed: {msg}"));
            return Some(msg);
        }
    };
    let desired = DesiredSchedule::from_requests(requests);
    let plan = ScheduleReconciliationPlan::new(&pending, &desired, &load_schedule_manifest());

    let cancel_error = cancel_notifications(&scheduler, &plan.to_cancel);
    let requests_to_schedule = desired.requests_for(&plan.to_schedule);
    let schedule_error = schedule_requests(&scheduler, &requests_to_schedule);
    let verify_error = verify_pending_request_ids(&scheduler, &desired.ids);
    let first_error = cancel_error.or(schedule_error).or(verify_error);

    if first_error.is_none() {
        save_schedule_manifest(ScheduleManifest {
            requests: desired.manifest_entries,
        });
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

#[cfg(target_os = "macos")]
struct DesiredSchedule {
    requests: Vec<NotificationRequest>,
    ids: BTreeSet<String>,
    manifest_entries: BTreeMap<String, ScheduleManifestRequest>,
}

#[cfg(target_os = "macos")]
impl DesiredSchedule {
    fn from_requests(requests: &[NotificationRequest]) -> Self {
        let now = Utc::now();
        let requests = requests
            .iter()
            .filter(|request| request.fire_at > now)
            .cloned()
            .collect::<Vec<_>>();
        let ids = requests
            .iter()
            .map(|request| request.id.clone())
            .collect::<BTreeSet<_>>();
        let manifest_entries = requests
            .iter()
            .map(|request| (request.id.clone(), schedule_manifest_entry(request)))
            .collect::<BTreeMap<_, _>>();

        Self {
            requests,
            ids,
            manifest_entries,
        }
    }

    fn requests_for(&self, ids: &BTreeSet<String>) -> Vec<NotificationRequest> {
        self.requests
            .iter()
            .filter(|request| ids.contains(&request.id))
            .cloned()
            .collect()
    }
}

#[cfg(target_os = "macos")]
struct ScheduleReconciliationPlan {
    to_cancel: BTreeSet<String>,
    to_schedule: BTreeSet<String>,
    kept_count: usize,
    desired_count: usize,
}

#[cfg(target_os = "macos")]
impl ScheduleReconciliationPlan {
    fn new(
        pending: &BTreeSet<String>,
        desired: &DesiredSchedule,
        manifest: &ScheduleManifest,
    ) -> Self {
        let stale = pending
            .difference(&desired.ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        let changed = desired
            .ids
            .intersection(pending)
            .filter(|id| manifest.requests.get(*id) != desired.manifest_entries.get(*id))
            .cloned()
            .collect::<BTreeSet<_>>();
        let missing = desired
            .ids
            .difference(pending)
            .cloned()
            .collect::<BTreeSet<_>>();

        let mut to_cancel = stale;
        to_cancel.extend(changed.iter().cloned());

        let mut to_schedule = missing;
        to_schedule.extend(changed);

        Self {
            kept_count: desired.ids.len().saturating_sub(to_schedule.len()),
            desired_count: desired.ids.len(),
            to_cancel,
            to_schedule,
        }
    }
}

#[cfg(target_os = "macos")]
fn managed_pending_ids(scheduler: &NotificationScheduler) -> Result<BTreeSet<String>, String> {
    scheduler
        .pending_ids()
        .map(|ids| {
            ids.into_iter()
                .filter(|id| is_managed_notification_id(id))
                .collect()
        })
        .map_err(|err| format!("{err}"))
}

#[cfg(target_os = "macos")]
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
            std::thread::sleep(StdDuration::from_millis(100));
            None
        }
        Err(err) => {
            let msg = format!("{err}");
            notif_log(&format!("cancel stale OS notifications failed: {msg}"));
            Some(msg)
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn schedule_os_notifications_replace_all(requests: &[NotificationRequest]) -> Option<String> {
    let scheduler = NotificationScheduler::new(APP_ID);
    if let Err(err) = scheduler.cancel_all() {
        let msg = format!("{err}");
        notif_log(&format!(
            "cancel_all pending OS notifications failed: {msg}"
        ));
        return Some(msg);
    }

    schedule_requests(&scheduler, requests)
}

fn schedule_requests(
    scheduler: &NotificationScheduler,
    requests: &[NotificationRequest],
) -> Option<String> {
    let mut first_error = None;
    let mut scheduled = 0;
    for request in requests {
        if request.fire_at <= Utc::now() {
            continue;
        }
        match scheduler.schedule(request) {
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
        requests.len()
    ));
    first_error
}

#[cfg(target_os = "macos")]
fn verify_pending_request_ids(
    scheduler: &NotificationScheduler,
    desired: &BTreeSet<String>,
) -> Option<String> {
    std::thread::sleep(StdDuration::from_millis(250));

    match managed_pending_ids(scheduler) {
        Ok(pending) => {
            let missing = desired
                .difference(&pending)
                .cloned()
                .collect::<Vec<String>>();
            let stale = pending
                .difference(&desired)
                .cloned()
                .collect::<Vec<String>>();

            if missing.is_empty() {
                notif_log(&format!(
                    "macOS retained {}/{} pending OS notification(s)",
                    pending.intersection(&desired).count(),
                    desired.len()
                ));
                if !stale.is_empty() {
                    notif_log(&format!(
                        "macOS pending schedule has {} stale request(s) after refresh",
                        stale.len()
                    ));
                }
                None
            } else {
                let preview = missing
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                notif_log(&format!(
                    "macOS did not retain {}/{} scheduled notification(s); missing: {preview}",
                    missing.len(),
                    desired.len()
                ));
                None
            }
        }
        Err(err) => {
            let msg = format!("{err}");
            notif_log(&format!(
                "pending OS notification verification failed: {msg}"
            ));
            Some(msg)
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Default, Serialize, Deserialize)]
struct ScheduleManifest {
    requests: BTreeMap<String, ScheduleManifestRequest>,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ScheduleManifestRequest {
    fingerprint: String,
    expires_at: Option<DateTime<Utc>>,
}

#[cfg(target_os = "macos")]
fn load_schedule_manifest() -> ScheduleManifest {
    let path = schedule_manifest_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ScheduleManifest::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn save_schedule_manifest(manifest: ScheduleManifest) {
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

#[cfg(target_os = "macos")]
fn save_item_schedule_manifest_entries(requests: &[NotificationRequest]) {
    if requests.is_empty() {
        return;
    }

    let mut manifest = load_schedule_manifest();
    for request in requests {
        manifest
            .requests
            .insert(request.id.clone(), schedule_manifest_entry(request));
    }
    save_schedule_manifest(manifest);
}

#[cfg(target_os = "macos")]
fn schedule_manifest_path() -> std::path::PathBuf {
    data_dir().join(SCHEDULE_MANIFEST_FILE)
}

#[cfg(target_os = "macos")]
fn request_fingerprint(request: &NotificationRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.id.as_bytes());
    hasher.update([0]);
    hasher.update(request.fire_at.to_rfc3339().as_bytes());
    hasher.update([0]);
    if let Some(expires_at) = request.expires_at {
        hasher.update(expires_at.to_rfc3339().as_bytes());
    }
    hasher.update([0]);
    hasher.update(request.title.as_bytes());
    hasher.update([0]);
    hasher.update(request.body.as_bytes());
    hasher.update([0]);
    if let Some(group) = &request.group {
        hasher.update(group.as_bytes());
    }
    hasher.update([0]);
    if let Some(category) = &request.category {
        hasher.update(category.as_bytes());
    }
    for (key, value) in &request.user_info {
        hasher.update([0]);
        hasher.update(key.as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    )
}

#[cfg(target_os = "macos")]
fn schedule_manifest_entry(request: &NotificationRequest) -> ScheduleManifestRequest {
    ScheduleManifestRequest {
        fingerprint: request_fingerprint(request),
        expires_at: request.expires_at,
    }
}

#[cfg(target_os = "macos")]
fn prune_expired_schedule_manifest(now: DateTime<Utc>) -> Vec<String> {
    let mut manifest = load_schedule_manifest();
    let mut expired = Vec::new();
    manifest.requests.retain(|id, request| {
        if request
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            expired.push(id.clone());
            false
        } else {
            true
        }
    });
    if !expired.is_empty() {
        save_schedule_manifest(manifest);
    }
    expired
}

#[cfg(target_os = "macos")]
fn is_managed_notification_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("knotq-") else {
        return false;
    };
    hex.len() == 16 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
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
        now - Duration::days(SCHEDULE_HORIZON_DAYS),
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
    let scan_start = target.trigger_at - Duration::days(370);
    let scan_end = target.trigger_at + Duration::days(370);
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

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_notification_id_matches_only_hashed_knotq_ids() {
        assert!(is_managed_notification_id("knotq-0123456789abcdef"));
        assert!(!is_managed_notification_id("knotq-test-scheduled-123"));
        assert!(!is_managed_notification_id("other-0123456789abcdef"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn request_fingerprint_changes_when_content_changes() {
        let now = Utc::now();
        let first = NotificationRequest::new("knotq-0123456789abcdef", now, "T", "B");
        let second = NotificationRequest::new("knotq-0123456789abcdef", now, "T2", "B");
        assert_ne!(request_fingerprint(&first), request_fingerprint(&second));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn schedule_manifest_entry_records_expiration() {
        let now = Utc::now();
        let expires_at = now + Duration::hours(1);
        let request = NotificationRequest::new("knotq-0123456789abcdef", now, "T", "B")
            .expires_at(Some(expires_at));

        assert_eq!(
            schedule_manifest_entry(&request).expires_at,
            Some(expires_at)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reconciliation_plan_keeps_changed_missing_and_stale_separate() {
        let unchanged = "knotq-0000000000000001".to_string();
        let changed = "knotq-0000000000000002".to_string();
        let missing = "knotq-0000000000000003".to_string();
        let stale = "knotq-0000000000000004".to_string();

        let pending = BTreeSet::from([unchanged.clone(), changed.clone(), stale.clone()]);
        let desired = DesiredSchedule {
            requests: Vec::new(),
            ids: BTreeSet::from([unchanged.clone(), changed.clone(), missing.clone()]),
            manifest_entries: BTreeMap::from([
                (unchanged.clone(), manifest_entry("same")),
                (changed.clone(), manifest_entry("new")),
                (missing.clone(), manifest_entry("new")),
            ]),
        };
        let manifest = ScheduleManifest {
            requests: BTreeMap::from([
                (unchanged.clone(), manifest_entry("same")),
                (changed.clone(), manifest_entry("old")),
            ]),
        };

        let plan = ScheduleReconciliationPlan::new(&pending, &desired, &manifest);

        assert_eq!(plan.to_cancel, BTreeSet::from([changed.clone(), stale]));
        assert_eq!(plan.to_schedule, BTreeSet::from([changed, missing]));
        assert_eq!(plan.kept_count, 1);
        assert_eq!(plan.desired_count, 3);
    }

    #[cfg(target_os = "macos")]
    fn manifest_entry(fingerprint: &str) -> ScheduleManifestRequest {
        ScheduleManifestRequest {
            fingerprint: fingerprint.to_string(),
            expires_at: None,
        }
    }
}
