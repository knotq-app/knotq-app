use crate::{AuthorizationStatus, Error, NotificationRequest, PlatformStatus, Result};
use std::collections::HashSet;
use std::env;
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
const WINDOWS_SHORTCUT_NAME: &str = "KnotQ.lnk";

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

pub fn schedule(app_id: &str, request: &NotificationRequest) -> Result<()> {
    let notifier = notifier(app_id)?;
    let xml = toast_xml(&request.title, &request.body);
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
    scheduled.SetTag(&id).map_err(windows_error)?;
    scheduled
        .SetGroup(&HSTRING::from(WINDOWS_GROUP))
        .map_err(windows_error)?;

    notifier.AddToSchedule(&scheduled).map_err(windows_error)
}

pub fn deliver_now(app_id: &str, request: &NotificationRequest) -> Result<()> {
    let notifier = notifier(app_id)?;
    let xml = toast_xml(&request.title, &request.body);
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
        .map(|id| windows_notification_id(id))
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
        history
            .RemoveGroupedTagWithId(&HSTRING::from(windows_notification_id(id)), &group, &app_id)
            .map_err(windows_error)?;
    }
    Ok(())
}

pub fn delivered_ids(_app_id: &str) -> Result<Vec<String>> {
    Ok(Vec::new())
}

pub fn remove_all_delivered(_app_id: &str) -> Result<()> {
    Ok(())
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
    Ok(PathBuf::from(path).join(WINDOWS_SHORTCUT_NAME))
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

fn toast_xml(title: &str, body: &str) -> String {
    format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>"#,
        escape_xml(title),
        escape_xml(body)
    )
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
}
