//! Shared plumbing for the notification module: identifiers, constants, the
//! durable schedule manifest on disk, and the small helpers every submodule
//! leans on (logging, id hashing, single-item workspaces).
use chrono::Utc;
use knotq_model::{Item, Scheme, SchemeId, Workspace};
use knotq_notifications::{
    NotificationLeadTimes, PlatformSchedulePolicy, PlatformStatus, ScheduleManifest,
};
use knotq_storage_json::data_dir;
use knotq_storage_json::NotificationDefaults;
use sha2::{Digest, Sha256};
use std::time::Duration as StdDuration;

#[cfg(target_os = "macos")]
const PLATFORM_OS_PENDING_LIMIT: usize = 16;
#[cfg(not(target_os = "macos"))]
const PLATFORM_OS_PENDING_LIMIT: usize = knotq_notifications::DEFAULT_DURABLE_NOTIFICATION_LIMIT;
pub(crate) const SCHEDULE_HORIZON_DAYS: i64 = 14;
const PLATFORM_OS_HARD_HORIZON: StdDuration = StdDuration::from_secs(32 * 24 * 60 * 60);
pub(crate) const APP_ID: &str = "com.enigmadux.knotq";
pub(crate) const CATEGORY_ID: &str = "knotq-reminder";
const SCHEDULE_MANIFEST_FILE: &str = "notification_schedule_manifest.json";
pub(crate) const NOTIFICATION_LOOKBACK_DAYS: i64 = 7;

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

pub(crate) fn base_schedule_policy() -> PlatformSchedulePolicy {
    PlatformSchedulePolicy::new(PLATFORM_OS_PENDING_LIMIT)
        .with_max_schedule_horizon(PLATFORM_OS_HARD_HORIZON)
}

pub(crate) fn background_schedule_policy() -> PlatformSchedulePolicy {
    let policy = base_schedule_policy();
    #[cfg(target_os = "macos")]
    {
        policy
            .with_add_interval(StdDuration::from_millis(150))
            .with_verify_delays(StdDuration::from_millis(500), StdDuration::from_millis(750))
    }
    #[cfg(not(target_os = "macos"))]
    {
        policy
    }
}

pub(crate) fn shutdown_schedule_policy() -> PlatformSchedulePolicy {
    base_schedule_policy()
}

pub(crate) fn load_schedule_manifest() -> ScheduleManifest {
    let path = schedule_manifest_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ScheduleManifest::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub(crate) fn save_schedule_manifest(manifest: &ScheduleManifest) {
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

pub(crate) fn os_notification_id(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    format!(
        "knotq-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    )
}

/// Hash a batch of notification keys to their stable OS identifiers.
pub(crate) fn keys_to_ids(keys: impl IntoIterator<Item = String>) -> Vec<String> {
    keys.into_iter()
        .map(|key| os_notification_id(&key))
        .collect()
}

/// Build a throwaway single-item workspace so the notification computation can
/// run against one item in isolation.
pub(crate) fn workspace_for_item(
    scheme_id: SchemeId,
    item: Item,
    scheme_is_daily: bool,
) -> Workspace {
    let mut workspace = Workspace::empty();
    let mut scheme = Scheme::new("", 0);
    scheme.id = scheme_id;
    scheme.items.push(item);
    workspace.schemes.insert(scheme_id, scheme);
    if scheme_is_daily {
        // Register the scheme as a daily queue (the date itself is irrelevant to
        // key derivation) so this synthetic workspace computes the same stable
        // "daily" notification-key fragment as the full-workspace passes.
        workspace
            .daily_queue
            .insert(chrono::NaiveDate::MIN, scheme_id);
    }
    workspace
}

pub(crate) fn platform_status_message(status: PlatformStatus) -> String {
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

pub(crate) fn lead_times(defaults: NotificationDefaults) -> NotificationLeadTimes {
    NotificationLeadTimes {
        reminder_offset_secs: 0,
        event_offset_secs: defaults.event_offset_secs,
        assignment_offset_secs: defaults.assignment_offset_secs,
    }
}
