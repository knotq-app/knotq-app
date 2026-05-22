#![allow(unexpected_cfgs)]

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::sync::Mutex;

pub const ACTION_SNOOZE_10_MINUTES: &str = "knotq.snooze.10m";
pub const ACTION_SNOOZE_1_HOUR: &str = "knotq.snooze.1h";
pub const ACTION_MARK_DONE: &str = "knotq.mark_done";

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct NotificationRequest {
    pub id: String,
    pub fire_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub title: String,
    pub body: String,
    pub group: Option<String>,
    pub category: Option<String>,
    pub user_info: BTreeMap<String, String>,
}

#[cfg(target_os = "linux")]
pub fn run_linux_notification_helper_from_env() -> bool {
    platform::run_helper_from_env()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationResponse {
    pub notification_id: String,
    pub action_id: String,
    pub user_info: BTreeMap<String, String>,
}

static NOTIFICATION_RESPONSES: Mutex<Vec<NotificationResponse>> = Mutex::new(Vec::new());
static NOTIFICATION_RESPONSE_LISTENERS: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
    Mutex::new(Vec::new());

pub fn take_notification_responses() -> Vec<NotificationResponse> {
    let Ok(mut responses) = NOTIFICATION_RESPONSES.lock() else {
        return Vec::new();
    };
    std::mem::take(&mut *responses)
}

#[doc(hidden)]
pub fn dispatch_response(response: NotificationResponse) {
    if let Ok(mut responses) = NOTIFICATION_RESPONSES.lock() {
        responses.push(response);
    }
    if let Ok(mut listeners) = NOTIFICATION_RESPONSE_LISTENERS.lock() {
        listeners.retain(|listener| listener());
    }
}

pub fn add_notification_response_listener(listener: impl Fn() -> bool + Send + Sync + 'static) {
    if let Ok(mut listeners) = NOTIFICATION_RESPONSE_LISTENERS.lock() {
        listeners.push(Box::new(listener));
    }
}

impl NotificationRequest {
    pub fn new(
        id: impl Into<String>,
        fire_at: DateTime<Utc>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            fire_at,
            expires_at: None,
            title: display_text_or(title.into(), "(untitled)"),
            body: display_text_or(body.into(), "KnotQ notification"),
            group: None,
            category: None,
            user_info: BTreeMap::new(),
        }
    }

    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    pub fn expires_at(mut self, expires_at: Option<DateTime<Utc>>) -> Self {
        self.expires_at = expires_at;
        self
    }

    pub fn user_info(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.user_info.insert(key.into(), value.into());
        self
    }
}

fn display_text_or(value: String, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformStatus {
    Available,
    Unavailable(&'static str),
    Unsupported(&'static str),
}

impl PlatformStatus {
    pub fn can_schedule(self) -> bool {
        matches!(self, Self::Available)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationStatus {
    NotDetermined,
    Denied,
    Authorized,
    Provisional,
    Ephemeral,
    Unknown,
}

impl AuthorizationStatus {
    pub fn can_deliver(self) -> bool {
        matches!(self, Self::Authorized | Self::Provisional | Self::Ephemeral)
    }

    pub fn unavailable_reason(self) -> Option<&'static str> {
        match self {
            Self::NotDetermined => Some("notification authorization has not been requested"),
            Self::Denied => Some("notification authorization denied"),
            Self::Authorized | Self::Provisional | Self::Ephemeral => None,
            Self::Unknown => Some("notification authorization status is unknown"),
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("scheduled notifications are unsupported on this platform: {0}")]
    Unsupported(&'static str),
    #[error("scheduled notifications are unavailable in this runtime: {0}")]
    Unavailable(&'static str),
    #[error("platform notification error: {0}")]
    Platform(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct NotificationScheduler {
    app_id: String,
}

impl NotificationScheduler {
    pub fn new(app_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
        }
    }

    pub fn platform_status(&self) -> PlatformStatus {
        platform::status()
    }

    pub fn request_authorization(&self) -> Result<()> {
        platform::request_authorization()
    }

    pub fn configure_notification_handling(&self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        platform::configure_notification_handling();
    }

    /// Fire-and-forget authorization request. Safe to call from any thread,
    /// including the main thread — does not block.
    pub fn request_authorization_nonblocking(&self) {
        #[cfg(target_os = "macos")]
        platform::request_authorization_nonblocking();
    }

    pub fn authorization_status(&self) -> Result<AuthorizationStatus> {
        platform::authorization_status()
    }

    pub fn deliver_now(&self, request: &NotificationRequest) -> Result<()> {
        platform::deliver_now(&self.app_id, request)
    }

    pub fn schedule(&self, request: &NotificationRequest) -> Result<()> {
        platform::schedule(&self.app_id, request)
    }

    /// Schedule multiple notifications, checking authorization only once.
    pub fn schedule_batch(
        &self,
        requests: &[&NotificationRequest],
        add_interval: std::time::Duration,
    ) -> Vec<Result<()>> {
        platform::schedule_batch(&self.app_id, requests, add_interval)
    }

    pub fn cancel(&self, ids: &[String]) -> Result<()> {
        platform::cancel(&self.app_id, ids)
    }

    pub fn cancel_all(&self) -> Result<()> {
        platform::cancel_all(&self.app_id)
    }

    pub fn pending_ids(&self) -> Result<Vec<String>> {
        platform::pending_ids(&self.app_id)
    }

    pub fn remove_delivered(&self, ids: &[String]) -> Result<()> {
        platform::remove_delivered(&self.app_id, ids)
    }

    pub fn delivered_ids(&self) -> Result<Vec<String>> {
        platform::delivered_ids(&self.app_id)
    }

    pub fn remove_all_delivered(&self) -> Result<()> {
        platform::remove_all_delivered(&self.app_id)
    }
}

#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod platform;

#[cfg(windows)]
#[path = "platform/windows.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod platform;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::{AuthorizationStatus, Error, NotificationRequest, PlatformStatus, Result};

    const REASON: &str =
        "the freedesktop notification protocol does not provide durable future scheduling";

    pub fn status() -> PlatformStatus {
        PlatformStatus::Unsupported(REASON)
    }

    pub fn request_authorization() -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    pub fn authorization_status() -> Result<AuthorizationStatus> {
        Err(Error::Unsupported(REASON))
    }

    pub fn schedule(_app_id: &str, _request: &NotificationRequest) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    pub fn deliver_now(_app_id: &str, _request: &NotificationRequest) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    pub fn cancel(_app_id: &str, _ids: &[String]) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    pub fn cancel_all(_app_id: &str) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    pub fn pending_ids(_app_id: &str) -> Result<Vec<String>> {
        Err(Error::Unsupported(REASON))
    }

    pub fn remove_delivered(_app_id: &str, _ids: &[String]) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    pub fn delivered_ids(_app_id: &str) -> Result<Vec<String>> {
        Err(Error::Unsupported(REASON))
    }

    pub fn remove_all_delivered(_app_id: &str) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    pub fn schedule_batch(
        _app_id: &str,
        requests: &[&NotificationRequest],
        _add_interval: std::time::Duration,
    ) -> Vec<Result<()>> {
        requests
            .iter()
            .map(|_| Err(Error::Unsupported(REASON)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn request_builder_sets_optional_fields() {
        let request =
            NotificationRequest::new("id", Utc::now() + Duration::hours(1), "title", "body")
                .group("group")
                .category("category")
                .user_info("scheme_id", "scheme");

        assert_eq!(request.id, "id");
        assert_eq!(request.expires_at, None);
        assert_eq!(request.group.as_deref(), Some("group"));
        assert_eq!(request.category.as_deref(), Some("category"));
        assert_eq!(
            request.user_info.get("scheme_id").map(String::as_str),
            Some("scheme")
        );
    }

    #[test]
    fn request_builder_prevents_blank_display_text() {
        let request = NotificationRequest::new("id", Utc::now() + Duration::hours(1), " \n\t ", "");

        assert_eq!(request.title, "(untitled)");
        assert_eq!(request.body, "KnotQ notification");
    }

    #[test]
    fn request_builder_sets_expiration() {
        let fire_at = Utc::now() + Duration::hours(1);
        let expires_at = fire_at + Duration::hours(2);
        let request =
            NotificationRequest::new("id", fire_at, "title", "body").expires_at(Some(expires_at));

        assert_eq!(request.expires_at, Some(expires_at));
    }

    #[test]
    fn response_queue_drains_all_responses() {
        dispatch_response(NotificationResponse {
            notification_id: "id".to_string(),
            action_id: ACTION_SNOOZE_10_MINUTES.to_string(),
            user_info: BTreeMap::new(),
        });
        assert_eq!(take_notification_responses().len(), 1);
        assert!(take_notification_responses().is_empty());
    }
}
