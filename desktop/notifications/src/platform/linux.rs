use crate::{
    dispatch_response, AuthorizationStatus, Error, NotificationRequest, NotificationResponse,
    PlatformStatus, Result, ACTION_MARK_DONE, NOTIFICATION_SNOOZE_ACTIONS,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration as StdDuration;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type as MessageType;
use zbus::zvariant::Value;
use zbus::MatchRule;

const NOTIFICATIONS_DESTINATION: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
const NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";
const HELPER_BUS_NAME: &str = "com.enigmadux.KnotQ.NotificationHelper";
const HELPER_PATH: &str = "/com/enigmadux/KnotQ/NotificationHelper";
const HELPER_INTERFACE: &str = "com.enigmadux.KnotQ.NotificationHelper";
const HELPER_SIGNAL_SCHEDULE_CHANGED: &str = "ScheduleChanged";
const HELPER_ARG: &str = "--knotq-notification-helper";
const APP_NAME: &str = "KnotQ";
const APP_ICON: &str = "knotq";
const DURABLE_STATE_FILE: &str = "linux_notification_schedule.json";
const DURABLE_LOCK_FILE: &str = "linux_notification_schedule.lock";
const AUTOSTART_FILE: &str = "knotq-notification-helper.desktop";
const HELPER_START_WAIT: StdDuration = StdDuration::from_millis(100);
const HELPER_START_ATTEMPTS: usize = 20;
const HELPER_IDLE_WAKE: StdDuration = StdDuration::from_secs(5 * 60);
const HELPER_MAX_SLEEP: StdDuration = StdDuration::from_secs(60);
const TIMER_CLOCK_REFRESH: StdDuration = StdDuration::from_secs(60 * 60);

static FALLBACK_STATE: OnceLock<Mutex<FallbackNotificationState>> = OnceLock::new();
static ACTION_LISTENER_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct FallbackNotificationState {
    pending: BTreeMap<String, PendingEntry>,
}

struct PendingEntry {
    cancel: Arc<TimerCancel>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct DurableNotificationState {
    pending: BTreeMap<String, NotificationRequest>,
    delivered: BTreeMap<String, DurableDeliveredNotification>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DurableDeliveredNotification {
    remote_id: u32,
    user_info: BTreeMap<String, String>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct TimerCancel {
    canceled: Mutex<bool>,
    condvar: Condvar,
}

#[derive(Default)]
struct HelperWake {
    generation: Mutex<u64>,
    condvar: Condvar,
}

struct FileLock(File);

impl TimerCancel {
    fn cancel(&self) {
        let Ok(mut canceled) = self.canceled.lock() else {
            return;
        };
        *canceled = true;
        self.condvar.notify_all();
    }

    fn wait_until(&self, fire_at: DateTime<Utc>) -> bool {
        let Ok(mut canceled) = self.canceled.lock() else {
            return false;
        };
        loop {
            if *canceled {
                return false;
            }

            let now = Utc::now();
            if now >= fire_at {
                return true;
            }

            let wait = fire_at
                .signed_duration_since(now)
                .to_std()
                .unwrap_or(StdDuration::ZERO)
                .min(TIMER_CLOCK_REFRESH);
            match self.condvar.wait_timeout(canceled, wait) {
                Ok((guard, _)) => canceled = guard,
                Err(_) => return false,
            }
        }
    }
}

impl HelperWake {
    fn wake(&self) {
        let Ok(mut generation) = self.generation.lock() else {
            return;
        };
        *generation = generation.wrapping_add(1);
        self.condvar.notify_all();
    }

    fn wait(&self, timeout: StdDuration) {
        let Ok(generation) = self.generation.lock() else {
            std::thread::sleep(timeout);
            return;
        };
        let current = *generation;
        let _ = self
            .condvar
            .wait_timeout_while(generation, timeout, |generation| *generation == current);
    }
}

impl FileLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(io_error)?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc == -1 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        Ok(Self(file))
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub fn status() -> PlatformStatus {
    match ensure_service_available() {
        Ok(()) => PlatformStatus::Available,
        Err(_) => PlatformStatus::Unavailable(
            "freedesktop notification service is unavailable on the session bus",
        ),
    }
}

pub fn request_authorization() -> Result<()> {
    // Linux desktop notifications do not have a common runtime permission prompt.
    ensure_service_available()
}

pub fn authorization_status() -> Result<AuthorizationStatus> {
    ensure_service_available()?;
    Ok(AuthorizationStatus::Authorized)
}

pub fn configure_notification_handling() {
    ensure_action_listener();
}

pub fn schedule(app_id: &str, request: &NotificationRequest) -> Result<()> {
    ensure_service_available()?;
    schedule_unchecked(app_id, request)
}

pub fn deliver_now(app_id: &str, request: &NotificationRequest) -> Result<()> {
    show_notification(app_id, request).map(|_| ())
}

pub fn cancel(_app_id: &str, ids: &[String]) -> Result<()> {
    cancel_durable_pending(ids)?;
    cancel_fallback_pending(ids);
    signal_helper_schedule_changed();
    Ok(())
}

pub fn cancel_all(_app_id: &str) -> Result<()> {
    with_durable_state(|state| {
        state.pending.clear();
    })?;
    cancel_all_fallback_pending();
    signal_helper_schedule_changed();
    Ok(())
}

pub fn pending_ids(_app_id: &str) -> Result<Vec<String>> {
    let mut ids = read_durable_state()?
        .pending
        .into_keys()
        .collect::<Vec<_>>();
    if let Ok(state) = fallback_state().lock() {
        ids.extend(state.pending.keys().cloned());
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

pub fn remove_delivered(_app_id: &str, ids: &[String]) -> Result<()> {
    let remote_ids = with_durable_state(|state| {
        ids.iter()
            .filter_map(|id| state.delivered.remove(id))
            .map(|entry| entry.remote_id)
            .collect::<Vec<_>>()
    })?;
    close_remote_notifications(remote_ids)
}

pub fn delivered_ids(_app_id: &str) -> Result<Vec<String>> {
    Ok(read_durable_state()?.delivered.into_keys().collect())
}

pub fn remove_all_delivered(_app_id: &str) -> Result<()> {
    let remote_ids = with_durable_state(|state| {
        let remote_ids = state
            .delivered
            .values()
            .map(|entry| entry.remote_id)
            .collect::<Vec<_>>();
        state.delivered.clear();
        remote_ids
    })?;
    close_remote_notifications(remote_ids)
}

pub fn schedule_batch(
    app_id: &str,
    requests: &[&NotificationRequest],
    add_interval: StdDuration,
) -> Vec<Result<()>> {
    if requests.is_empty() {
        return Vec::new();
    }
    if let Err(err) = ensure_service_available() {
        return requests.iter().map(|_| Err(err.clone())).collect();
    }

    let now = Utc::now();
    let mut results = vec![None; requests.len()];
    let mut future_requests = Vec::new();

    for (idx, request) in requests.iter().enumerate() {
        if request.fire_at <= now {
            results[idx] = Some(deliver_now(app_id, request));
            if idx + 1 < requests.len() && !add_interval.is_zero() {
                std::thread::sleep(add_interval);
            }
        } else {
            future_requests.push((idx, (*request).clone()));
        }
    }

    if !future_requests.is_empty() {
        let helper_requests = future_requests
            .iter()
            .map(|(_, request)| request.clone())
            .collect::<Vec<_>>();
        match schedule_with_helper_batch(&helper_requests) {
            Ok(()) => {
                let ids = helper_requests
                    .iter()
                    .map(|request| request.id.clone())
                    .collect::<Vec<_>>();
                cancel_fallback_pending(&ids);
                for (idx, _) in &future_requests {
                    results[*idx] = Some(Ok(()));
                }
            }
            Err(err) => {
                eprintln!("KnotQ notification helper unavailable, using in-app timers: {err}");
                for (pos, (idx, request)) in future_requests.iter().enumerate() {
                    results[*idx] = Some(schedule_fallback_timer(app_id, request));
                    if pos + 1 < future_requests.len() && !add_interval.is_zero() {
                        std::thread::sleep(add_interval);
                    }
                }
            }
        }
    }

    results
        .into_iter()
        .map(|result| result.unwrap_or(Ok(())))
        .collect()
}

pub fn run_helper_from_env() -> bool {
    if !std::env::args().any(|arg| arg == HELPER_ARG) {
        return false;
    }
    if let Err(err) = run_helper() {
        eprintln!("KnotQ notification helper failed: {err}");
        std::process::exit(1);
    }
    true
}

fn schedule_unchecked(app_id: &str, request: &NotificationRequest) -> Result<()> {
    if request.fire_at <= Utc::now() {
        return deliver_now(app_id, request);
    }

    if let Err(err) = schedule_with_helper_batch(std::slice::from_ref(request)) {
        eprintln!("KnotQ notification helper unavailable, using in-app timer: {err}");
        return schedule_fallback_timer(app_id, request);
    }

    cancel_fallback_pending(std::slice::from_ref(&request.id));
    Ok(())
}

fn schedule_with_helper_batch(requests: &[NotificationRequest]) -> Result<()> {
    let ids = requests
        .iter()
        .map(|request| request.id.clone())
        .collect::<Vec<_>>();
    with_durable_state(|state| {
        prune_durable_state(state, Utc::now());
        for request in requests {
            state.pending.insert(request.id.clone(), request.clone());
        }
    })?;
    if let Err(err) = install_autostart_entry() {
        eprintln!("failed to install KnotQ notification helper autostart entry: {err}");
    }
    if let Err(err) = ensure_helper_running() {
        let _ = cancel_durable_pending(&ids);
        return Err(err);
    }
    signal_helper_schedule_changed();
    Ok(())
}

fn schedule_fallback_timer(app_id: &str, request: &NotificationRequest) -> Result<()> {
    ensure_action_listener();
    let request = request.clone();
    let app_id = app_id.to_string();
    let cancel = Arc::new(TimerCancel::default());
    {
        let mut state = fallback_state()
            .lock()
            .map_err(|_| Error::Unavailable("Linux fallback notification state is unavailable"))?;
        if let Some(old) = state.pending.insert(
            request.id.clone(),
            PendingEntry {
                cancel: cancel.clone(),
            },
        ) {
            old.cancel.cancel();
        }
    }

    let request_id = request.id.clone();
    let timer_request = request.clone();
    match std::thread::Builder::new()
        .name("knotq-linux-notification-timer".to_string())
        .spawn({
            let cancel = cancel.clone();
            move || run_fallback_timer(app_id, timer_request, cancel)
        }) {
        Ok(_) => Ok(()),
        Err(err) => {
            cancel_fallback_pending(&[request_id]);
            Err(io_error(err))
        }
    }
}

fn run_fallback_timer(app_id: String, request: NotificationRequest, cancel: Arc<TimerCancel>) {
    if !cancel.wait_until(request.fire_at) {
        return;
    }
    if request
        .expires_at
        .is_some_and(|expires_at| expires_at <= Utc::now())
    {
        remove_fallback_pending_if_current(&request.id, &cancel);
        return;
    }
    if !remove_fallback_pending_if_current(&request.id, &cancel) {
        return;
    }
    if let Err(err) = show_notification(&app_id, &request) {
        eprintln!("failed to deliver Linux notification {}: {err}", request.id);
    }
}

fn remove_fallback_pending_if_current(id: &str, cancel: &Arc<TimerCancel>) -> bool {
    let Ok(mut state) = fallback_state().lock() else {
        return false;
    };
    let Some(entry) = state.pending.get(id) else {
        return false;
    };
    if !Arc::ptr_eq(&entry.cancel, cancel) {
        return false;
    }
    state.pending.remove(id);
    true
}

fn cancel_fallback_pending(ids: &[String]) {
    let canceled = {
        let Ok(mut state) = fallback_state().lock() else {
            return;
        };
        ids.iter()
            .filter_map(|id| state.pending.remove(id))
            .collect::<Vec<_>>()
    };
    for entry in canceled {
        entry.cancel.cancel();
    }
}

fn cancel_all_fallback_pending() {
    let pending = {
        let Ok(mut state) = fallback_state().lock() else {
            return;
        };
        std::mem::take(&mut state.pending)
    };
    for entry in pending.into_values() {
        entry.cancel.cancel();
    }
}

fn show_notification(app_id: &str, request: &NotificationRequest) -> Result<u32> {
    ensure_action_listener();
    let connection = Connection::session().map_err(dbus_error)?;
    let proxy = notifications_proxy(&connection)?;
    let replaces_id = delivered_remote_id(&request.id).unwrap_or(0);
    let actions = notification_actions(request);
    let hints = notification_hints(app_id, request);
    let expire_timeout = expire_timeout_ms(request);
    let remote_id = proxy
        .call(
            "Notify",
            &(
                APP_NAME,
                replaces_id,
                APP_ICON,
                request.title.as_str(),
                request.body.as_str(),
                actions,
                hints,
                expire_timeout,
            ),
        )
        .map_err(dbus_error)?;
    record_delivered(request, remote_id);
    Ok(remote_id)
}

fn notification_actions(request: &NotificationRequest) -> Vec<&'static str> {
    if !request.user_info.contains_key("scheme_id")
        || !request.user_info.contains_key("item_id")
        || !request.user_info.contains_key("occurrence_json")
        || !request.user_info.contains_key("trigger_at")
    {
        return Vec::new();
    }

    let mut actions = Vec::with_capacity((NOTIFICATION_SNOOZE_ACTIONS.len() + 1) * 2);
    for action in NOTIFICATION_SNOOZE_ACTIONS {
        actions.push(action.action_id);
        actions.push(action.label);
    }
    actions.push(ACTION_MARK_DONE);
    actions.push("Mark done");
    actions
}

fn notification_hints(
    app_id: &str,
    request: &NotificationRequest,
) -> BTreeMap<&'static str, Value<'static>> {
    let mut hints = BTreeMap::new();
    hints.insert("desktop-entry", Value::new(app_id.to_string()));
    hints.insert("urgency", Value::new(1u8));
    if let Some(category) = &request.category {
        hints.insert("category", Value::new(category.clone()));
    }
    hints
}

fn expire_timeout_ms(request: &NotificationRequest) -> i32 {
    let Some(expires_at) = request.expires_at else {
        return -1;
    };
    let Ok(duration) = expires_at.signed_duration_since(Utc::now()).to_std() else {
        return 1;
    };
    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX)
}

fn record_delivered(request: &NotificationRequest, remote_id: u32) {
    let _ = with_durable_state(|state| {
        state.delivered.insert(
            request.id.clone(),
            DurableDeliveredNotification {
                remote_id,
                user_info: request.user_info.clone(),
                expires_at: request.expires_at,
            },
        );
    });
}

fn delivered_remote_id(id: &str) -> Option<u32> {
    read_durable_state()
        .ok()
        .and_then(|state| state.delivered.get(id).map(|entry| entry.remote_id))
}

fn close_remote_notifications(remote_ids: Vec<u32>) -> Result<()> {
    if remote_ids.is_empty() {
        return Ok(());
    }

    let connection = Connection::session().map_err(dbus_error)?;
    let proxy = notifications_proxy(&connection)?;
    let mut first_error = None;
    for remote_id in remote_ids {
        if let Err(err) = proxy.call::<_, _, ()>("CloseNotification", &(remote_id,)) {
            if first_error.is_none() {
                first_error = Some(dbus_error(err));
            }
        }
    }
    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn run_helper() -> Result<()> {
    ensure_service_available()?;
    let owner_connection = Connection::session().map_err(dbus_error)?;
    owner_connection
        .request_name(HELPER_BUS_NAME)
        .map_err(dbus_error)?;

    ensure_action_listener();
    let wake = Arc::new(HelperWake::default());
    spawn_schedule_listener(wake.clone())?;

    loop {
        let now = Utc::now();
        for request in take_due_requests(now)? {
            if let Err(err) = show_notification("com.enigmadux.knotq", &request) {
                eprintln!(
                    "failed to deliver scheduled Linux notification {}: {err}",
                    request.id
                );
            }
        }

        let wait = next_pending_wait(Utc::now())?
            .unwrap_or(HELPER_IDLE_WAKE)
            .min(HELPER_MAX_SLEEP);
        wake.wait(wait);
    }
}

fn take_due_requests(now: DateTime<Utc>) -> Result<Vec<NotificationRequest>> {
    with_durable_state(|state| {
        prune_durable_state(state, now);
        let due_ids = state
            .pending
            .iter()
            .filter(|(_, request)| request.fire_at <= now)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut due = due_ids
            .into_iter()
            .filter_map(|id| state.pending.remove(&id))
            .collect::<Vec<_>>();
        due.sort_by_key(|request| request.fire_at);
        due
    })
}

fn next_pending_wait(now: DateTime<Utc>) -> Result<Option<StdDuration>> {
    with_durable_state(|state| {
        prune_durable_state(state, now);
        state
            .pending
            .values()
            .filter(|request| request.fire_at > now)
            .map(|request| {
                request
                    .fire_at
                    .signed_duration_since(now)
                    .to_std()
                    .unwrap_or(StdDuration::ZERO)
            })
            .min()
    })
}

fn prune_durable_state(state: &mut DurableNotificationState, now: DateTime<Utc>) {
    state.pending.retain(|_, request| {
        request
            .expires_at
            .map_or(true, |expires_at| expires_at > now)
    });
    state
        .delivered
        .retain(|_, entry| entry.expires_at.map_or(true, |expires_at| expires_at > now));
}

fn install_autostart_entry() -> Result<()> {
    let exe = std::env::current_exe().map_err(io_error)?;
    let path = autostart_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=KnotQ Notification Helper\nExec={} {}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        desktop_exec_arg(&exe),
        HELPER_ARG
    );
    write_atomic(&path, entry.as_bytes())
}

fn ensure_helper_running() -> Result<()> {
    if helper_has_owner()? {
        return Ok(());
    }

    let exe = std::env::current_exe().map_err(io_error)?;
    Command::new(exe)
        .arg(HELPER_ARG)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io_error)?;

    for _ in 0..HELPER_START_ATTEMPTS {
        std::thread::sleep(HELPER_START_WAIT);
        if helper_has_owner()? {
            return Ok(());
        }
    }

    Err(Error::Unavailable("notification helper did not start"))
}

fn helper_has_owner() -> Result<bool> {
    let connection = Connection::session().map_err(dbus_error)?;
    let proxy = Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .map_err(dbus_error)?;
    proxy
        .call("NameHasOwner", &(HELPER_BUS_NAME,))
        .map_err(dbus_error)
}

fn signal_helper_schedule_changed() {
    let Ok(connection) = Connection::session() else {
        return;
    };
    let _ = connection.emit_signal(
        None::<&str>,
        HELPER_PATH,
        HELPER_INTERFACE,
        HELPER_SIGNAL_SCHEDULE_CHANGED,
        &(),
    );
}

fn spawn_schedule_listener(wake: Arc<HelperWake>) -> Result<()> {
    std::thread::Builder::new()
        .name("knotq-linux-notification-helper-signals".to_string())
        .spawn(move || {
            if let Err(err) = listen_for_schedule_signals(wake) {
                eprintln!("Linux notification helper schedule listener stopped: {err}");
            }
        })
        .map_err(io_error)
        .map(|_| ())
}

fn listen_for_schedule_signals(wake: Arc<HelperWake>) -> Result<()> {
    let connection = Connection::session().map_err(dbus_error)?;
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .path(HELPER_PATH)
        .map_err(dbus_error)?
        .interface(HELPER_INTERFACE)
        .map_err(dbus_error)?
        .member(HELPER_SIGNAL_SCHEDULE_CHANGED)
        .map_err(dbus_error)?
        .build();
    let mut messages =
        MessageIterator::for_match_rule(rule, &connection, Some(64)).map_err(dbus_error)?;
    for message in &mut messages {
        if message.is_ok() {
            wake.wake();
        }
    }
    Ok(())
}

fn ensure_action_listener() {
    if ACTION_LISTENER_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    if std::thread::Builder::new()
        .name("knotq-linux-notification-actions".to_string())
        .spawn(|| {
            if let Err(err) = listen_for_notification_actions() {
                eprintln!("Linux notification action listener stopped: {err}");
            }
            ACTION_LISTENER_STARTED.store(false, Ordering::Release);
        })
        .is_err()
    {
        ACTION_LISTENER_STARTED.store(false, Ordering::Release);
    }
}

fn listen_for_notification_actions() -> Result<()> {
    let connection = Connection::session().map_err(dbus_error)?;
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .path(NOTIFICATIONS_PATH)
        .map_err(dbus_error)?
        .interface(NOTIFICATIONS_INTERFACE)
        .map_err(dbus_error)?
        .build();
    let mut messages =
        MessageIterator::for_match_rule(rule, &connection, Some(64)).map_err(dbus_error)?;

    for message in &mut messages {
        let message = message.map_err(dbus_error)?;
        match message.header().member().map(|member| member.as_str()) {
            Some("ActionInvoked") => {
                let Ok((remote_id, action_id)) = message.body().deserialize::<(u32, String)>()
                else {
                    continue;
                };
                dispatch_action(remote_id, action_id);
            }
            Some("NotificationClosed") => {
                let Ok((remote_id, _reason)) = message.body().deserialize::<(u32, u32)>() else {
                    continue;
                };
                forget_remote_notification(remote_id);
            }
            _ => {}
        }
    }
    Ok(())
}

fn dispatch_action(remote_id: u32, action_id: String) {
    let response = read_durable_state().ok().and_then(|state| {
        state
            .delivered
            .iter()
            .find(|(_, entry)| entry.remote_id == remote_id)
            .map(|(id, entry)| NotificationResponse {
                notification_id: id.clone(),
                action_id,
                user_info: entry.user_info.clone(),
            })
    });
    if let Some(response) = response {
        dispatch_response(response);
    }
}

fn forget_remote_notification(remote_id: u32) {
    let _ = with_durable_state(|state| {
        let remove = state
            .delivered
            .iter()
            .find(|(_, entry)| entry.remote_id == remote_id)
            .map(|(id, _)| id.clone());
        if let Some(id) = remove {
            state.delivered.remove(&id);
        }
    });
}

fn cancel_durable_pending(ids: &[String]) -> Result<()> {
    let ids = ids.iter().collect::<BTreeSet<_>>();
    with_durable_state(|state| {
        state.pending.retain(|id, _| !ids.contains(id));
    })
}

fn read_durable_state() -> Result<DurableNotificationState> {
    let _lock = FileLock::acquire(&durable_lock_path())?;
    read_durable_state_unlocked()
}

fn with_durable_state<T>(update: impl FnOnce(&mut DurableNotificationState) -> T) -> Result<T> {
    let _lock = FileLock::acquire(&durable_lock_path())?;
    let mut state = read_durable_state_unlocked()?;
    let result = update(&mut state);
    write_durable_state_unlocked(&state)?;
    Ok(result)
}

fn read_durable_state_unlocked() -> Result<DurableNotificationState> {
    let path = durable_state_path();
    match fs::read_to_string(&path) {
        Ok(raw) if raw.trim().is_empty() => Ok(DurableNotificationState::default()),
        Ok(raw) => serde_json::from_str(&raw).map_err(|err| Error::Platform(err.to_string())),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(DurableNotificationState::default()),
        Err(err) => Err(io_error(err)),
    }
}

fn write_durable_state_unlocked(state: &DurableNotificationState) -> Result<()> {
    let path = durable_state_path();
    let raw = serde_json::to_vec_pretty(state).map_err(|err| Error::Platform(err.to_string()))?;
    write_atomic(&path, &raw)
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).map_err(io_error)?;
    fs::rename(&tmp, path).map_err(io_error)
}

fn ensure_service_available() -> Result<()> {
    let connection = Connection::session().map_err(dbus_error)?;
    let proxy = notifications_proxy(&connection)?;
    let _: (String, String, String, String) = proxy
        .call("GetServerInformation", &())
        .map_err(dbus_error)?;
    Ok(())
}

fn notifications_proxy(connection: &Connection) -> Result<Proxy<'_>> {
    Proxy::new(
        connection,
        NOTIFICATIONS_DESTINATION,
        NOTIFICATIONS_PATH,
        NOTIFICATIONS_INTERFACE,
    )
    .map_err(dbus_error)
}

fn fallback_state() -> &'static Mutex<FallbackNotificationState> {
    FALLBACK_STATE.get_or_init(|| Mutex::new(FallbackNotificationState::default()))
}

fn durable_state_path() -> PathBuf {
    data_dir().join(DURABLE_STATE_FILE)
}

fn durable_lock_path() -> PathBuf {
    data_dir().join(DURABLE_LOCK_FILE)
}

fn autostart_path() -> PathBuf {
    config_dir().join("autostart").join(AUTOSTART_FILE)
}

fn data_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/share/knotq");
    }
    PathBuf::from(".")
}

fn config_dir() -> PathBuf {
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        if !config_home.trim().is_empty() {
            return PathBuf::from(config_home);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config");
    }
    PathBuf::from(".")
}

fn desktop_exec_arg(path: &Path) -> String {
    let escaped = path
        .as_os_str()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

fn dbus_error(error: zbus::Error) -> Error {
    Error::Platform(error.to_string())
}

fn io_error(error: std::io::Error) -> Error {
    Error::Platform(error.to_string())
}
