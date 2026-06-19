#[cfg(windows)]
use std::collections::BTreeMap;

#[cfg(any(target_os = "linux", windows))]
use crate::NotificationRequest;

pub(crate) const ACTION_TARGET_SCHEME_ID_KEY: &str = "scheme_id";
pub(crate) const ACTION_TARGET_ITEM_ID_KEY: &str = "item_id";
pub(crate) const ACTION_TARGET_OCCURRENCE_JSON_KEY: &str = "occurrence_json";
pub(crate) const ACTION_TARGET_TRIGGER_AT_KEY: &str = "trigger_at";

#[cfg(any(target_os = "linux", windows))]
const ACTION_TARGET_USER_INFO_KEYS: &[&str] = &[
    ACTION_TARGET_SCHEME_ID_KEY,
    ACTION_TARGET_ITEM_ID_KEY,
    ACTION_TARGET_OCCURRENCE_JSON_KEY,
    ACTION_TARGET_TRIGGER_AT_KEY,
];

#[cfg(any(target_os = "linux", windows))]
pub(crate) fn request_has_action_payload(request: &NotificationRequest) -> bool {
    ACTION_TARGET_USER_INFO_KEYS
        .iter()
        .all(|key| request.user_info.contains_key(*key))
}

#[cfg(windows)]
pub(crate) fn action_payload_pairs<'a>(
    request: &'a NotificationRequest,
) -> impl Iterator<Item = (&'static str, &'a str)> + 'a {
    ACTION_TARGET_USER_INFO_KEYS.iter().filter_map(|&key| {
        request
            .user_info
            .get(key)
            .map(|value| (key, value.as_str()))
    })
}

#[cfg(windows)]
pub(crate) fn action_payload_from_params(
    params: &BTreeMap<String, String>,
) -> Option<BTreeMap<String, String>> {
    ACTION_TARGET_USER_INFO_KEYS
        .iter()
        .map(|&key| {
            params
                .get(key)
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect()
}
