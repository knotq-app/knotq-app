use crate::{AuthorizationStatus, Error, NotificationRequest, PlatformStatus, Result};
use block::{Block, ConcreteBlock};
use objc::runtime::{Object, BOOL, NO};
use objc::{class, msg_send, sel, sel_impl};
use std::sync::mpsc;
use std::time::Duration as StdDuration;

#[path = "macos_delegate.rs"]
mod delegate;
#[path = "macos_support.rs"]
mod support;

use delegate::configure_notification_center;
use support::{
    bundle_unavailable_reason, notification_center, ns_error_description, nsstring, nsstring_array,
    user_info_dictionary, AutoreleasePool,
};

#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}

const AUTHORIZATION_OPTION_BADGE: u64 = 1 << 0;
const AUTHORIZATION_OPTION_SOUND: u64 = 1 << 1;
const AUTHORIZATION_OPTION_ALERT: u64 = 1 << 2;
const UN_NOTIFICATION_INTERRUPTION_LEVEL_TIME_SENSITIVE: usize = 2;

pub fn status() -> PlatformStatus {
    unsafe {
        let _pool = AutoreleasePool::new();
        match bundle_unavailable_reason() {
            Some(reason) => PlatformStatus::Unavailable(reason),
            None => PlatformStatus::Available,
        }
    }
}

pub fn request_authorization() -> Result<()> {
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        configure_notification_center(center);
        let options: u64 =
            AUTHORIZATION_OPTION_BADGE | AUTHORIZATION_OPTION_SOUND | AUTHORIZATION_OPTION_ALERT;
        let (tx, rx) = mpsc::channel();
        let completion = ConcreteBlock::new(move |granted: BOOL, error: *mut Object| {
            let result = authorization_result(granted, error);
            let _ = tx.send(result);
        })
        .copy();
        let completion_ptr = &*completion as *const Block<(BOOL, *mut Object), ()>;
        let _: () = msg_send![
            center,
            requestAuthorizationWithOptions: options
            completionHandler: completion_ptr
        ];

        match rx.recv_timeout(StdDuration::from_secs(120)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                std::mem::forget(completion);
                Err(Error::Unavailable("notification authorization timed out"))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Unavailable(
                "notification authorization callback dropped",
            )),
        }
    }
}

pub fn configure_notification_handling() {
    unsafe {
        let _pool = AutoreleasePool::new();
        if let Ok(center) = notification_center() {
            configure_notification_center(center);
        }
    }
}

/// Fire-and-forget authorization request dispatched onto the main queue.
/// macOS requires this to run on the main thread to show the system
/// permission dialog.  Does not block.
pub fn request_authorization_nonblocking() {
    extern "C" {
        // `dispatch_get_main_queue()` is a macro; the real symbol is
        // `_dispatch_main_q`.
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }

    extern "C" fn do_request_auth(_ctx: *mut std::ffi::c_void) {
        unsafe {
            let pool = AutoreleasePool::new();
            let Ok(center) = notification_center() else {
                return;
            };
            configure_notification_center(center);
            let options: u64 = AUTHORIZATION_OPTION_BADGE
                | AUTHORIZATION_OPTION_SOUND
                | AUTHORIZATION_OPTION_ALERT;
            let completion =
                ConcreteBlock::new(move |_granted: BOOL, _error: *mut Object| {}).copy();
            let completion_ptr = &*completion as *const Block<(BOOL, *mut Object), ()>;
            let _: () = msg_send![
                center,
                requestAuthorizationWithOptions: options
                completionHandler: completion_ptr
            ];
            std::mem::forget(completion);
            drop(pool);
        }
    }

    unsafe {
        let queue = &_dispatch_main_q as *const std::ffi::c_void;
        dispatch_async_f(queue, std::ptr::null_mut(), do_request_auth);
    }
}

pub fn authorization_status() -> Result<AuthorizationStatus> {
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let (tx, rx) = mpsc::channel();
        let completion = ConcreteBlock::new(move |settings: *mut Object| {
            let result = notification_settings_status(settings);
            let _ = tx.send(result);
        })
        .copy();
        let completion_ptr = &*completion as *const Block<(*mut Object,), ()>;
        let _: () = msg_send![
            center,
            getNotificationSettingsWithCompletionHandler: completion_ptr
        ];

        match rx.recv_timeout(StdDuration::from_secs(5)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                std::mem::forget(completion);
                Err(Error::Unavailable(
                    "notification settings callback timed out",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(Error::Unavailable("notification settings callback dropped"))
            }
        }
    }
}

pub fn deliver_now(_app_id: &str, request: &NotificationRequest) -> Result<()> {
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let status = authorization_status()?;
        if !status.can_deliver() {
            return Err(Error::Unavailable(
                status
                    .unavailable_reason()
                    .unwrap_or("notification authorization unavailable"),
            ));
        }
        add_notification_request(center, request, std::ptr::null_mut())
    }
}

pub fn schedule(_app_id: &str, request: &NotificationRequest) -> Result<()> {
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let status = authorization_status()?;
        if !status.can_deliver() {
            return Err(Error::Unavailable(
                status
                    .unavailable_reason()
                    .unwrap_or("notification authorization unavailable"),
            ));
        }
        let trigger = calendar_trigger(request.fire_at);
        log_trigger_debug(&request.id, trigger);
        add_notification_request(center, request, trigger)
    }
}

pub fn cancel(_app_id: &str, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let identifiers = nsstring_array(ids);
        let _: () = msg_send![
            center,
            removePendingNotificationRequestsWithIdentifiers: identifiers
        ];
        Ok(())
    }
}

pub fn cancel_all(_app_id: &str) -> Result<()> {
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let _: () = msg_send![center, removeAllPendingNotificationRequests];
        Ok(())
    }
}

pub fn pending_ids(_app_id: &str) -> Result<Vec<String>> {
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let (tx, rx) = mpsc::channel();
        let completion = ConcreteBlock::new(move |requests: *mut Object| {
            let ids = pending_request_ids(requests);
            let _ = tx.send(ids);
        })
        .copy();
        let completion_ptr = &*completion as *const Block<(*mut Object,), ()>;
        let _: () = msg_send![
            center,
            getPendingNotificationRequestsWithCompletionHandler: completion_ptr
        ];

        match rx.recv_timeout(StdDuration::from_secs(5)) {
            Ok(ids) => Ok(ids),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                std::mem::forget(completion);
                Err(Error::Unavailable(
                    "pending notification callback timed out",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(Error::Unavailable("pending notification callback dropped"))
            }
        }
    }
}

pub fn remove_delivered(_app_id: &str, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let identifiers = nsstring_array(ids);
        let _: () = msg_send![
            center,
            removeDeliveredNotificationsWithIdentifiers: identifiers
        ];
        Ok(())
    }
}

pub fn delivered_ids(_app_id: &str) -> Result<Vec<String>> {
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let (tx, rx) = mpsc::channel();
        let completion = ConcreteBlock::new(move |notifications: *mut Object| {
            let ids = delivered_notification_request_ids(notifications);
            let _ = tx.send(ids);
        })
        .copy();
        let completion_ptr = &*completion as *const Block<(*mut Object,), ()>;
        let _: () = msg_send![
            center,
            getDeliveredNotificationsWithCompletionHandler: completion_ptr
        ];

        match rx.recv_timeout(StdDuration::from_secs(5)) {
            Ok(ids) => Ok(ids),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                std::mem::forget(completion);
                Err(Error::Unavailable(
                    "delivered notification callback timed out",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Unavailable(
                "delivered notification callback dropped",
            )),
        }
    }
}

pub fn remove_all_delivered(_app_id: &str) -> Result<()> {
    unsafe {
        let _pool = AutoreleasePool::new();
        let center = notification_center()?;
        let _: () = msg_send![center, removeAllDeliveredNotifications];
        Ok(())
    }
}

unsafe fn add_notification_request(
    center: *mut Object,
    request: &NotificationRequest,
    trigger: *mut Object,
) -> Result<()> {
    let content: *mut Object = msg_send![class!(UNMutableNotificationContent), new];
    let _: *mut Object = msg_send![content, autorelease];
    let title = nsstring(&request.title);
    let body = nsstring(&request.body);
    let _: () = msg_send![content, setTitle: title];
    let _: () = msg_send![content, setBody: body];
    set_time_sensitive_interruption_level(content);

    let sound: *mut Object = msg_send![class!(UNNotificationSound), defaultSound];
    if !sound.is_null() {
        let _: () = msg_send![content, setSound: sound];
    }
    if let Some(category) = &request.category {
        let category = nsstring(category);
        let _: () = msg_send![content, setCategoryIdentifier: category];
    }
    if let Some(group) = &request.group {
        let group = nsstring(group);
        let _: () = msg_send![content, setThreadIdentifier: group];
    }
    if let Some(user_info) = user_info_dictionary(request) {
        let _: () = msg_send![content, setUserInfo: user_info];
    }

    let identifier = nsstring(&request.id);
    let notification_request: *mut Object = msg_send![
        class!(UNNotificationRequest),
        requestWithIdentifier: identifier
        content: content
        trigger: trigger
    ];
    wait_for_add_request(center, notification_request)
}

unsafe fn set_time_sensitive_interruption_level(content: *mut Object) {
    let responds: BOOL = msg_send![content, respondsToSelector: sel!(setInterruptionLevel:)];
    if responds != NO {
        let _: () = msg_send![
            content,
            setInterruptionLevel: UN_NOTIFICATION_INTERRUPTION_LEVEL_TIME_SENSITIVE
        ];
    }
}

unsafe fn calendar_trigger(fire_at: chrono::DateTime<chrono::Utc>) -> *mut Object {
    let timestamp = fire_at.timestamp_millis() as f64 / 1000.0;
    let date: *mut Object = msg_send![
        class!(NSDate),
        dateWithTimeIntervalSince1970: timestamp
    ];
    let calendar: *mut Object = msg_send![class!(NSCalendar), currentCalendar];
    let units: u64 = (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7);
    let components: *mut Object = msg_send![
        calendar,
        components: units
        fromDate: date
    ];
    msg_send![
        class!(UNCalendarNotificationTrigger),
        triggerWithDateMatchingComponents: components
        repeats: NO
    ]
}

unsafe fn log_trigger_debug(request_id: &str, trigger: *mut Object) {
    let description = if trigger.is_null() {
        "nil trigger".to_string()
    } else {
        let next_date: *mut Object = msg_send![trigger, nextTriggerDate];
        let next_description: *mut Object = if next_date.is_null() {
            std::ptr::null_mut()
        } else {
            msg_send![next_date, description]
        };
        support::nsstring_to_string(next_description)
            .unwrap_or_else(|| "nil nextTriggerDate".into())
    };
    platform_log(&format!(
        "macOS trigger request_id={request_id} next={description}"
    ));
}

fn platform_log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/knotq-notif.log")
    {
        let _ = writeln!(f, "[{}] {}", chrono::Utc::now().format("%H:%M:%S"), msg);
    }
}

unsafe fn pending_request_ids(requests: *mut Object) -> Vec<String> {
    if requests.is_null() {
        return Vec::new();
    }

    let count: usize = msg_send![requests, count];
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let request: *mut Object = msg_send![requests, objectAtIndex: index];
        if request.is_null() {
            continue;
        }
        let identifier: *mut Object = msg_send![request, identifier];
        if let Some(id) = support::nsstring_to_string(identifier) {
            ids.push(id);
        }
    }
    ids
}

unsafe fn delivered_notification_request_ids(notifications: *mut Object) -> Vec<String> {
    if notifications.is_null() {
        return Vec::new();
    }

    let count: usize = msg_send![notifications, count];
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let notification: *mut Object = msg_send![notifications, objectAtIndex: index];
        if notification.is_null() {
            continue;
        }
        let request: *mut Object = msg_send![notification, request];
        if request.is_null() {
            continue;
        }
        let identifier: *mut Object = msg_send![request, identifier];
        if let Some(id) = support::nsstring_to_string(identifier) {
            ids.push(id);
        }
    }
    ids
}

unsafe fn wait_for_add_request(
    center: *mut Object,
    notification_request: *mut Object,
) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let completion = ConcreteBlock::new(move |error: *mut Object| {
        let result = if error.is_null() {
            Ok(())
        } else {
            Err(Error::Platform(unsafe { ns_error_description(error) }))
        };
        let _ = tx.send(result);
    })
    .copy();
    let completion_handler = &*completion as *const Block<(*mut Object,), ()>;
    let _: () = msg_send![
        center,
        addNotificationRequest: notification_request
        withCompletionHandler: completion_handler
    ];
    match rx.recv_timeout(StdDuration::from_secs(5)) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            std::mem::forget(completion);
            Err(Error::Unavailable(
                "notification request callback timed out",
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(Error::Unavailable("notification request callback dropped"))
        }
    }
}

unsafe fn authorization_result(granted: BOOL, error: *mut Object) -> Result<()> {
    if !error.is_null() {
        let description = ns_error_description(error);
        Err(Error::Platform(description))
    } else if granted == NO {
        Err(Error::Unavailable("notification authorization denied"))
    } else {
        Ok(())
    }
}

unsafe fn notification_settings_status(settings: *mut Object) -> Result<AuthorizationStatus> {
    if settings.is_null() {
        return Err(Error::Unavailable("UNNotificationSettings returned nil"));
    }
    let status: isize = msg_send![settings, authorizationStatus];
    Ok(match status {
        0 => AuthorizationStatus::NotDetermined,
        1 => AuthorizationStatus::Denied,
        2 => AuthorizationStatus::Authorized,
        3 => AuthorizationStatus::Provisional,
        4 => AuthorizationStatus::Ephemeral,
        _ => AuthorizationStatus::Unknown,
    })
}
