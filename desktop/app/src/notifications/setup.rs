//! Authorization requests and platform availability checks.
use knotq_notifications::{AuthorizationStatus, NotificationScheduler, PlatformStatus};
use std::sync::atomic::{AtomicBool, Ordering};

use super::common::{notif_log, platform_status_message, APP_ID};

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
