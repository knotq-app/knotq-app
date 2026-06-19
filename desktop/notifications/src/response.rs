use std::collections::BTreeMap;
use std::sync::Mutex;

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
