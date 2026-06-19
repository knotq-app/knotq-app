use crate::action_payload::{
    action_payload_from_params, action_payload_pairs, request_has_action_payload,
};
use crate::{
    dispatch_response, AuthorizationStatus, Error, NotificationRequest, NotificationResponse,
    PlatformStatus, Result, ACTION_MARK_DONE, ACTION_SNOOZE_10_MINUTES,
    NOTIFICATION_SNOOZE_ACTIONS,
};
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fmt::Write;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use windows::core::{Interface, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::DateTime;
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, SHGetKnownFolderPath, SetCurrentProcessExplicitAppUserModelID,
    ShellLink,
};
use windows::UI::Notifications::{
    ScheduledToastNotification, ToastNotification, ToastNotificationManager, ToastNotifier,
};

const WINDOWS_GROUP: &str = "knotq";
const WINDOWS_NOTIFICATION_ID_LEN: usize = 15;
const WINDOWS_SHORTCUT_DIR: &str = "KnotQ";
const WINDOWS_SHORTCUT_NAME: &str = "KnotQ.lnk";
const WINDOWS_ACTION_SNOOZE_SELECTED: &str = "knotq.snooze.selected";
const WINDOWS_PROTOCOL: &str = "knotq://notification";
const WINDOWS_SNOOZE_INPUT_ID: &str = "snooze_action_id";

pub fn status() -> PlatformStatus {
    PlatformStatus::Available
}

pub fn request_authorization() -> Result<()> {
    // Windows desktop notifications do not have a runtime permission prompt.
    Ok(())
}

pub fn authorization_status() -> Result<AuthorizationStatus> {
    // Windows desktop notifications do not have a runtime permission prompt.
    Ok(AuthorizationStatus::Authorized)
}

pub fn configure_notification_handling() {
    if let Some(response) = notification_response_from_windows_args(env::args().skip(1)) {
        dispatch_response(response);
    }
}

pub fn schedule(app_id: &str, request: &NotificationRequest) -> Result<()> {
    let notifier = notifier(app_id)?;
    let xml = toast_xml(request);
    let document = XmlDocument::new().map_err(windows_error)?;
    document
        .LoadXml(&HSTRING::from(xml))
        .map_err(windows_error)?;

    let scheduled = ScheduledToastNotification::CreateScheduledToastNotification(
        &document,
        windows_time(request.fire_at),
    )
    .map_err(windows_error)?;

    let id = HSTRING::from(windows_notification_id(&request.id));
    scheduled.SetId(&id).map_err(windows_error)?;
    scheduled
        .SetTag(&HSTRING::from(&request.id))
        .map_err(windows_error)?;
    scheduled
        .SetGroup(&HSTRING::from(WINDOWS_GROUP))
        .map_err(windows_error)?;

    notifier.AddToSchedule(&scheduled).map_err(windows_error)
}

pub fn deliver_now(app_id: &str, request: &NotificationRequest) -> Result<()> {
    let notifier = notifier(app_id)?;
    let xml = toast_xml(request);
    let document = XmlDocument::new().map_err(windows_error)?;
    document
        .LoadXml(&HSTRING::from(xml))
        .map_err(windows_error)?;

    let notification =
        ToastNotification::CreateToastNotification(&document).map_err(windows_error)?;
    notifier.Show(&notification).map_err(windows_error)
}

pub fn cancel(app_id: &str, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let ids = ids
        .iter()
        .flat_map(|id| [id.clone(), windows_notification_id(id)])
        .collect::<HashSet<_>>();
    let notifier = notifier(app_id)?;
    let scheduled = notifier
        .GetScheduledToastNotifications()
        .map_err(windows_error)?;
    for index in 0..scheduled.Size().map_err(windows_error)? {
        let notification = scheduled.GetAt(index).map_err(windows_error)?;
        let id = notification.Id().map_err(windows_error)?.to_string_lossy();
        let tag = notification
            .Tag()
            .map(|tag| tag.to_string_lossy())
            .unwrap_or_default();
        if ids.contains(id.as_str()) || ids.contains(tag.as_str()) {
            notifier
                .RemoveFromSchedule(&notification)
                .map_err(windows_error)?;
        }
    }
    Ok(())
}

pub fn cancel_all(app_id: &str) -> Result<()> {
    let notifier = notifier(app_id)?;
    let scheduled = notifier
        .GetScheduledToastNotifications()
        .map_err(windows_error)?;
    // Collect first to avoid index shifting when removing while iterating.
    let mut to_remove = Vec::new();
    for index in 0..scheduled.Size().map_err(windows_error)? {
        to_remove.push(scheduled.GetAt(index).map_err(windows_error)?);
    }
    for notification in to_remove {
        notifier
            .RemoveFromSchedule(&notification)
            .map_err(windows_error)?;
    }
    Ok(())
}

pub fn pending_ids(app_id: &str) -> Result<Vec<String>> {
    let notifier = notifier(app_id)?;
    let scheduled = notifier
        .GetScheduledToastNotifications()
        .map_err(windows_error)?;
    let mut ids = Vec::new();
    for index in 0..scheduled.Size().map_err(windows_error)? {
        let notification = scheduled.GetAt(index).map_err(windows_error)?;
        let tag = notification
            .Tag()
            .map(|tag| tag.to_string_lossy())
            .unwrap_or_default();
        if !tag.is_empty() {
            ids.push(tag);
            continue;
        }
        let id = notification.Id().map_err(windows_error)?.to_string_lossy();
        if !id.is_empty() {
            ids.push(id);
        }
    }
    Ok(ids)
}

pub fn remove_delivered(app_id: &str, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let app_id = HSTRING::from(app_id);
    let history = ToastNotificationManager::History().map_err(windows_error)?;
    let group = HSTRING::from(WINDOWS_GROUP);
    for id in ids {
        let full_id = HSTRING::from(id);
        let windows_id = HSTRING::from(windows_notification_id(id));
        history
            .RemoveGroupedTagWithId(&full_id, &group, &app_id)
            .map_err(windows_error)?;
        let _ = history.RemoveGroupedTagWithId(&windows_id, &group, &app_id);
    }
    Ok(())
}

pub fn delivered_ids(_app_id: &str) -> Result<Vec<String>> {
    Ok(Vec::new())
}

pub fn remove_all_delivered(_app_id: &str) -> Result<()> {
    Ok(())
}

pub fn schedule_batch(
    app_id: &str,
    requests: &[&NotificationRequest],
    add_interval: std::time::Duration,
) -> Vec<Result<()>> {
    requests
        .iter()
        .enumerate()
        .map(|(i, req)| {
            let result = schedule(app_id, req);
            if i + 1 < requests.len() && !add_interval.is_zero() {
                std::thread::sleep(add_interval);
            }
            result
        })
        .collect()
}

fn notifier(app_id: &str) -> Result<ToastNotifier> {
    if app_id.trim().is_empty() {
        return Err(Error::Unavailable(
            "Windows scheduled notifications require an AppUserModelID",
        ));
    }
    ensure_app_identity(app_id)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))
        .map_err(windows_error)
}

fn ensure_app_identity(app_id: &str) -> Result<()> {
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(app_id)).map_err(windows_error)?;
        let _com = ComGuard::new()?;
        create_start_menu_shortcut(app_id)
    }
}

unsafe fn create_start_menu_shortcut(app_id: &str) -> Result<()> {
    let shortcut_path = start_menu_shortcut_path()?;
    if let Some(parent) = shortcut_path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }

    let exe_path = env::current_exe().map_err(io_error)?;
    let shell_link: IShellLinkW =
        CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(windows_error)?;

    shell_link
        .SetPath(&HSTRING::from(path_to_string(&exe_path)))
        .map_err(windows_error)?;
    shell_link
        .SetDescription(&HSTRING::from("KnotQ"))
        .map_err(windows_error)?;
    if let Some(parent) = exe_path.parent() {
        shell_link
            .SetWorkingDirectory(&HSTRING::from(path_to_string(&parent.to_path_buf())))
            .map_err(windows_error)?;
    }
    shell_link
        .SetIconLocation(&HSTRING::from(path_to_string(&exe_path)), 0)
        .map_err(windows_error)?;

    let property_store: IPropertyStore = shell_link.cast().map_err(windows_error)?;
    let mut app_id_value = propvariant_from_string(app_id)?;
    property_store
        .SetValue(&PKEY_AppUserModel_ID, &app_id_value)
        .map_err(windows_error)?;
    property_store.Commit().map_err(windows_error)?;
    PropVariantClear(&mut app_id_value).map_err(windows_error)?;

    let persist_file: IPersistFile = shell_link.cast().map_err(windows_error)?;
    persist_file
        .Save(&HSTRING::from(path_to_string(&shortcut_path)), true)
        .map_err(windows_error)
}

unsafe fn start_menu_shortcut_path() -> Result<PathBuf> {
    let programs = SHGetKnownFolderPath(&FOLDERID_Programs, Default::default(), None)
        .map_err(windows_error)?;
    let path = programs
        .to_string()
        .map_err(|err| Error::Platform(format!("invalid Start Menu path: {err}")))?;
    CoTaskMemFree(Some(programs.0.cast()));
    Ok(PathBuf::from(path)
        .join(WINDOWS_SHORTCUT_DIR)
        .join(WINDOWS_SHORTCUT_NAME))
}

fn path_to_string(path: &PathBuf) -> String {
    path.as_os_str().to_string_lossy().into_owned()
}

unsafe fn propvariant_from_string(value: &str) -> Result<PROPVARIANT> {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();
    let ptr = CoTaskMemAlloc(bytes) as *mut u16;
    if ptr.is_null() {
        return Err(Error::Platform(
            "failed to allocate AppUserModelID property".to_string(),
        ));
    }
    ptr.copy_from_nonoverlapping(wide.as_ptr(), wide.len());

    Ok(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    pwszVal: windows::core::PWSTR(ptr),
                },
            }),
        },
    })
}

struct ComGuard {
    uninitialize: bool,
}

impl ComGuard {
    unsafe fn new() -> Result<Self> {
        let result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if result == S_OK || result == S_FALSE {
            Ok(Self { uninitialize: true })
        } else if result == RPC_E_CHANGED_MODE {
            Ok(Self {
                uninitialize: false,
            })
        } else {
            Err(windows_error(result.into()))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninitialize {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

fn windows_time(time: chrono::DateTime<chrono::Utc>) -> DateTime {
    const WINDOWS_EPOCH_OFFSET_SECS: i64 = 11_644_473_600;
    let ticks = (time.timestamp() + WINDOWS_EPOCH_OFFSET_SECS) * 10_000_000
        + i64::from(time.timestamp_subsec_nanos() / 100);
    DateTime {
        UniversalTime: ticks,
    }
}

fn toast_xml(request: &NotificationRequest) -> String {
    let launch_args = if request_has_action_payload(request) {
        windows_activation_uri(request, "")
    } else {
        String::new()
    };
    let actions = windows_actions_xml(request);
    format!(
        r#"<toast scenario="reminder" launch="{}"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>{}<audio src="ms-winsoundevent:Notification.Reminder"/></toast>"#,
        escape_xml(&launch_args),
        escape_xml(&request.title),
        escape_xml(&request.body),
        actions,
    )
}

fn windows_actions_xml(request: &NotificationRequest) -> String {
    if !request_has_action_payload(request) {
        return String::new();
    }

    let mut xml = String::from(r#"<actions>"#);
    xml.push_str(r#"<input id="snooze_action_id" type="selection" defaultInput=""#);
    xml.push_str(ACTION_SNOOZE_10_MINUTES);
    xml.push_str(r#"">"#);
    for action in NOTIFICATION_SNOOZE_ACTIONS {
        let _ = write!(
            xml,
            r#"<selection id="{}" content="{}"/>"#,
            escape_xml(action.action_id),
            escape_xml(action.label),
        );
    }
    xml.push_str(r#"</input>"#);
    let _ = write!(
        xml,
        r#"<action content="Snooze" arguments="{}" activationType="protocol" hint-inputId="{}"/>"#,
        escape_xml(&windows_activation_uri(
            request,
            WINDOWS_ACTION_SNOOZE_SELECTED,
        )),
        WINDOWS_SNOOZE_INPUT_ID,
    );
    let _ = write!(
        xml,
        r#"<action content="Mark done" arguments="{}" activationType="protocol"/>"#,
        escape_xml(&windows_activation_uri(request, ACTION_MARK_DONE)),
    );
    xml.push_str("</actions>");
    xml
}

fn windows_activation_uri(request: &NotificationRequest, action_id: &str) -> String {
    format!(
        "{}?{}",
        WINDOWS_PROTOCOL,
        windows_activation_query(request, action_id)
    )
}

fn windows_activation_query(request: &NotificationRequest, action_id: &str) -> String {
    let mut params = vec![
        ("knotq_notification_action", "1"),
        ("notification_id", request.id.as_str()),
        ("action_id", action_id),
    ];
    params.extend(action_payload_pairs(request));
    params
        .into_iter()
        .map(|(key, value)| format!("{}={}", key, percent_encode_argument(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn notification_response_from_windows_args(
    args: impl IntoIterator<Item = String>,
) -> Option<NotificationResponse> {
    let args = args.into_iter().collect::<Vec<_>>();
    for arg in &args {
        if let Some(response) = notification_response_from_windows_activation(arg) {
            return Some(response);
        }
    }
    for pair in args.windows(2) {
        if pair[0].contains("ToastActivated") || pair[0].contains("NotificationActivated") {
            if let Some(response) = notification_response_from_windows_activation(&pair[1]) {
                return Some(response);
            }
        }
    }
    notification_response_from_windows_activation(&args.join(" "))
}

fn notification_response_from_windows_activation(raw: &str) -> Option<NotificationResponse> {
    let query = windows_activation_query_from_arg(raw)?;
    let params = parse_activation_query(query);
    if params.get("knotq_notification_action").map(String::as_str) != Some("1") {
        return None;
    }

    let mut action_id = params.get("action_id")?.to_string();
    if action_id == WINDOWS_ACTION_SNOOZE_SELECTED {
        action_id = params
            .get(WINDOWS_SNOOZE_INPUT_ID)
            .cloned()
            .filter(|action| !action.trim().is_empty())
            .unwrap_or_else(|| ACTION_SNOOZE_10_MINUTES.to_string());
    }

    let notification_id = params.get("notification_id")?.to_string();
    let user_info = action_payload_from_params(&params)?;

    Some(NotificationResponse {
        notification_id,
        action_id,
        user_info,
    })
}

fn windows_activation_query_from_arg(raw: &str) -> Option<&str> {
    let trimmed = raw.trim().trim_matches('"');
    let start = trimmed
        .find(WINDOWS_PROTOCOL)
        .map(|index| index + WINDOWS_PROTOCOL.len())
        .or_else(|| trimmed.find("knotq_notification_action=").map(|_| 0))?;
    let candidate = &trimmed[start..];
    Some(candidate.strip_prefix('?').unwrap_or(candidate))
}

fn parse_activation_query(query: &str) -> BTreeMap<String, String> {
    query
        .split(['&', ';'])
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((
                percent_decode_argument(key),
                percent_decode_argument(value.trim_matches('"')),
            ))
        })
        .collect()
}

fn percent_encode_argument(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn percent_decode_argument(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hi = hex_value(bytes[index + 1]);
                let lo = hex_value(bytes[index + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    decoded.push((hi << 4) | lo);
                    index += 3;
                    continue;
                }
                decoded.push(bytes[index]);
                index += 1;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn windows_notification_id(id: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mask = (1u64 << (WINDOWS_NOTIFICATION_ID_LEN * 4)) - 1;
    format!(
        "{:0width$x}",
        hash & mask,
        width = WINDOWS_NOTIFICATION_ID_LEN
    )
}

fn windows_error(error: windows::core::Error) -> Error {
    Error::Platform(error.message())
}

fn io_error(error: std::io::Error) -> Error {
    Error::Platform(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ACTION_SNOOZE_6_HOURS;
    use chrono::{Duration, Utc};

    #[test]
    fn windows_notification_id_is_short_and_stable() {
        let key = "2d58d58c-d0e4-419d-bd33-75ae98e5bc9a|d803c921-6797-4788-a43d-80e390aef354|123|a";
        let id = windows_notification_id(key);

        assert_eq!(id.len(), WINDOWS_NOTIFICATION_ID_LEN);
        assert_eq!(id, windows_notification_id(key));
    }

    #[test]
    fn windows_notification_id_never_exceeds_limit() {
        let keys = [
            "",
            "short",
            "2d58d58c-d0e4-419d-bd33-75ae98e5bc9a|d803c921-6797-4788-a43d-80e390aef354|123|a",
            "ffffffff-ffff-ffff-ffff-ffffffffffff|ffffffff-ffff-ffff-ffff-ffffffffffff|999999999|assignment",
        ];

        for key in keys {
            assert_eq!(
                windows_notification_id(key).len(),
                WINDOWS_NOTIFICATION_ID_LEN
            );
        }
    }

    #[test]
    fn toast_xml_includes_snooze_selector_and_mark_done_action() {
        let request = actionable_request();

        let xml = toast_xml(&request);

        assert!(xml.contains(r#"<input id="snooze_action_id" type="selection""#));
        assert!(xml.contains(r#"<selection id="knotq.snooze.6h" content="Snooze 6 hours"/>"#));
        assert!(xml.contains(
            r#"<selection id="knotq.snooze.tomorrow_morning" content="Tomorrow Morning"/>"#
        ));
        assert!(xml.contains(r#"<action content="Snooze""#));
        assert!(xml.contains(r#"activationType="protocol""#));
        assert!(xml.contains("knotq://notification?"));
        assert!(xml.contains(r#"action_id=knotq.snooze.selected"#));
        assert!(xml.contains(r#"<action content="Mark done""#));
        assert!(xml.contains(r#"action_id=knotq.mark_done"#));
    }

    #[test]
    fn toast_xml_omits_actions_without_target_payload() {
        let request =
            NotificationRequest::new("id", Utc::now() + Duration::hours(1), "title", "body");

        let xml = toast_xml(&request);

        assert!(!xml.contains("<actions>"));
    }

    #[test]
    fn notification_response_parses_protocol_activation() {
        let request = actionable_request();
        let uri = windows_activation_uri(&request, ACTION_MARK_DONE);

        let response = notification_response_from_windows_activation(&uri).unwrap();

        assert_eq!(response.notification_id, request.id);
        assert_eq!(response.action_id, ACTION_MARK_DONE);
        assert_eq!(
            response
                .user_info
                .get("occurrence_json")
                .map(String::as_str),
            Some(r#"{"kind":"single"}"#)
        );
    }

    #[test]
    fn notification_response_uses_selected_snooze_input() {
        let request = actionable_request();
        let uri = format!(
            "{}&{}={}",
            windows_activation_uri(&request, WINDOWS_ACTION_SNOOZE_SELECTED),
            WINDOWS_SNOOZE_INPUT_ID,
            ACTION_SNOOZE_6_HOURS
        );

        let response = notification_response_from_windows_activation(&uri).unwrap();

        assert_eq!(response.action_id, ACTION_SNOOZE_6_HOURS);
    }

    #[test]
    fn notification_response_defaults_snooze_selector_to_10_minutes() {
        let request = actionable_request();
        let uri = windows_activation_uri(&request, WINDOWS_ACTION_SNOOZE_SELECTED);

        let response = notification_response_from_windows_activation(&uri).unwrap();

        assert_eq!(response.action_id, ACTION_SNOOZE_10_MINUTES);
    }

    fn actionable_request() -> NotificationRequest {
        NotificationRequest::new(
            "note & id",
            Utc::now() + Duration::hours(1),
            "title",
            "body",
        )
        .user_info("scheme_id", "scheme")
        .user_info("item_id", "item")
        .user_info("occurrence_json", r#"{"kind":"single"}"#)
        .user_info("trigger_at", "2026-06-05T09:00:00Z")
    }
}
