use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct NotificationResponse {
    pub notification_id: String,
    pub action_id: String,
    pub user_info: BTreeMap<String, String>,
}

static NOTIFICATION_RESPONSES: Mutex<Vec<NotificationResponse>> = Mutex::new(Vec::new());
static NOTIFICATION_RESPONSE_LISTENERS: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
    Mutex::new(Vec::new());

pub fn take_notification_responses() -> Vec<NotificationResponse> {
    let drained = NOTIFICATION_RESPONSES
        .lock()
        .map(|mut responses| std::mem::take(&mut *responses))
        .unwrap_or_default();
    #[cfg(target_os = "linux")]
    {
        let mut drained = drained;
        drained.extend(crate::platform_provider::take_linux_durable_responses());
        drained
    }
    #[cfg(not(target_os = "linux"))]
    {
        drained
    }
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

#[cfg(target_os = "linux")]
pub(crate) fn wake_notification_response_listeners() {
    if let Ok(mut listeners) = NOTIFICATION_RESPONSE_LISTENERS.lock() {
        listeners.retain(|listener| listener());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ACTION_SNOOZE_10_MINUTES;

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
