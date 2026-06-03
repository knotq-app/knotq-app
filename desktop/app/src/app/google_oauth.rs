use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration as StdDuration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use base64::Engine as _;
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use gpui::Context;
use knotq_model::{
    CalendarDateTime, CalendarProvider, ExternalItemSource, FolderId, GoogleOAuthAccount,
    ImportedCalendarSource, Item, ItemMarker, NodeRef, Scheme, SchemeId, SchemeSource, Workspace,
};
use rand::distributions::{Alphanumeric, DistString};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::workspace_ops::{
    emails_match, google_account_matches_calendar_source, google_calendar_source_target_label,
};
use super::{
    GoogleCalendarPickerAccount, GoogleCalendarPickerCalendar, GoogleCalendarPickerState,
    GoogleCalendarPickerStatus, GoogleOAuthStatus, KnotQApp, NoticeModal, SidebarContextMenu,
    SidebarContextTarget,
};

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CALENDAR_LIST_URL: &str =
    "https://www.googleapis.com/calendar/v3/users/me/calendarList";
const GOOGLE_EVENTS_BASE_URL: &str = "https://www.googleapis.com/calendar/v3/calendars";
const GOOGLE_OAUTH_SCOPES: &[&str] = &[
    "openid",
    "email",
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
    "https://www.googleapis.com/auth/calendar.events.readonly",
];
const IMPORTED_GOOGLE_CALENDAR_SCHEME_NAME: &str = "Google Calendar";
const GOOGLE_CALENDAR_BACKGROUND_SYNC_INTERVAL_SECS: u64 = 5 * 60;

#[derive(Clone)]
struct GoogleOAuthConfig {
    client_id: String,
    client_secret: Option<String>,
}

#[derive(Clone)]
struct ExistingGoogleCalendarSource {
    account_id: String,
    account_email: Option<String>,
    calendar_id: String,
    sync_token: Option<String>,
}

struct GoogleCalendarImportResult {
    accounts: Vec<GoogleOAuthAccount>,
    calendars: Vec<ImportedGoogleCalendar>,
    failures: Vec<String>,
}

struct GoogleCalendarPickerLoadResult {
    accounts: Vec<GoogleOAuthAccount>,
    picker_accounts: Vec<GoogleCalendarPickerAccount>,
}

struct ImportedGoogleCalendar {
    account_id: String,
    account_email: Option<String>,
    calendar_id: String,
    name: String,
    color_index: u8,
    sync_token: Option<String>,
    full_sync: bool,
    items: Vec<Item>,
    deleted: Vec<GoogleExternalEventKey>,
}

#[derive(Clone, Copy)]
enum GoogleCalendarImportMode {
    MissingOnly,
    ExistingOnly,
}

struct GoogleCalendarApplyResult {
    content_changed: bool,
}

struct GoogleCalendarBackgroundSnapshot {
    config: GoogleOAuthConfig,
    accounts: Vec<GoogleOAuthAccount>,
    sources: Vec<ExistingGoogleCalendarSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GoogleExternalEventKey {
    event_id: String,
    instance_id: Option<String>,
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    scope: Option<String>,
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct GoogleIdClaims {
    sub: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCalendarListResponse {
    next_page_token: Option<String>,
    items: Vec<GoogleCalendarListEntry>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCalendarListEntry {
    id: String,
    summary: Option<String>,
    summary_override: Option<String>,
    background_color: Option<String>,
    hidden: Option<bool>,
    selected: Option<bool>,
    deleted: Option<bool>,
    primary: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEventsResponse {
    next_page_token: Option<String>,
    next_sync_token: Option<String>,
    items: Vec<GoogleEvent>,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEvent {
    id: String,
    status: Option<String>,
    summary: Option<String>,
    start: Option<GoogleEventDateTime>,
    end: Option<GoogleEventDateTime>,
    updated: Option<DateTime<Utc>>,
    recurrence: Option<Vec<String>>,
    recurring_event_id: Option<String>,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEventDateTime {
    date: Option<NaiveDate>,
    date_time: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct GoogleApiError {
    status: Option<u16>,
    message: String,
}

impl std::fmt::Display for GoogleApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GoogleApiError {}

impl KnotQApp {
    pub(crate) fn open_google_calendar_picker(
        &mut self,
        parent: FolderId,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_context_menu = Some(SidebarContextMenu {
            target: SidebarContextTarget::GoogleCalendarPicker { parent },
            position,
        });
        self.google_calendar_picker = Some(GoogleCalendarPickerState {
            parent,
            status: GoogleCalendarPickerStatus::Loading,
        });

        let accounts = self.settings.google_accounts.clone();
        let sources = active_google_calendar_sources(&self.workspace);
        if accounts.is_empty() {
            self.google_calendar_picker = Some(GoogleCalendarPickerState {
                parent,
                status: GoogleCalendarPickerStatus::Loaded {
                    accounts: Vec::new(),
                },
            });
            self.google_calendar_picker_task = None;
            cx.notify();
            return;
        }

        let config = match google_oauth_config_for_existing_accounts(&accounts) {
            Ok(config) => config,
            Err(err) => {
                self.google_calendar_picker = Some(GoogleCalendarPickerState {
                    parent,
                    status: GoogleCalendarPickerStatus::Error(format!("{err:#}")),
                });
                self.google_calendar_picker_task = None;
                cx.notify();
                return;
            }
        };

        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = run_google_calendar_picker_load(config, accounts, sources)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_picker_load(parent, result, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_picker_load(
                                    parent,
                                    Err("Google Calendar selector worker stopped".to_string()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_calendar_picker_task = Some(task);
        cx.notify();
    }

    pub(crate) fn spawn_google_calendar_sync_task(cx: &mut Context<Self>) -> gpui::Task<()> {
        cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| loop {
                cx.background_executor()
                    .timer(StdDuration::from_secs(
                        GOOGLE_CALENDAR_BACKGROUND_SYNC_INTERVAL_SECS,
                    ))
                    .await;

                let snapshot = weak
                    .update(cx, |app, _cx| app.google_calendar_background_snapshot())
                    .ok()
                    .flatten();
                let Some(snapshot) = snapshot else {
                    continue;
                };

                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = run_google_calendar_background_sync(
                        snapshot.config,
                        snapshot.accounts,
                        snapshot.sources,
                    )
                    .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_background_sync(result, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            eprintln!("background Google Calendar sync worker stopped");
                            break;
                        }
                    }
                }
            },
        )
    }

    pub(crate) fn start_google_calendar_import(
        &mut self,
        parent: FolderId,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress) {
            return;
        }

        let config = match google_oauth_config_from_env() {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Google Calendar import failed: {err:#}");
                self.google_oauth_status = GoogleOAuthStatus::Error;
                cx.notify();
                return;
            }
        };
        let accounts = self.settings.google_accounts.clone();
        let sources = active_google_calendar_sources(&self.workspace);

        self.google_oauth_status = GoogleOAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = run_google_calendar_import(config, accounts, sources)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_import(parent, result, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_import(
                                    parent,
                                    Err("Google OAuth worker stopped".to_string()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_oauth_task = Some(task);
        cx.notify();
    }

    pub(crate) fn start_google_calendar_import_calendar(
        &mut self,
        parent: FolderId,
        account_id: String,
        calendar_id: String,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress) {
            return;
        }

        let Some(account) = self
            .settings
            .google_accounts
            .iter()
            .find(|account| account.account_id == account_id)
            .cloned()
        else {
            self.show_google_calendar_error(
                "Google Calendar import",
                "That Google account is not connected locally anymore.".to_string(),
            );
            self.google_oauth_status = GoogleOAuthStatus::Error;
            cx.notify();
            return;
        };

        if let Some(scheme_id) =
            find_google_calendar_scheme_for_account(&self.workspace, &account, &calendar_id)
        {
            self.open_scheme(scheme_id, None);
            cx.notify();
            return;
        }
        if let Some(scheme_id) = find_archived_google_calendar_scheme_for_account(
            &self.workspace,
            &account,
            &calendar_id,
        ) {
            self.restore_deleted_scheme(scheme_id, cx);
            return;
        }

        let config = match google_oauth_config_for_existing_accounts(std::slice::from_ref(&account))
        {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Google Calendar import failed: {err:#}");
                self.show_google_calendar_error("Google Calendar import", format!("{err:#}"));
                self.google_oauth_status = GoogleOAuthStatus::Error;
                cx.notify();
                return;
            }
        };
        let sources = active_google_calendar_sources(&self.workspace);

        self.google_oauth_status = GoogleOAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = run_google_calendar_import_existing_account_calendar(
                        config,
                        account,
                        sources,
                        calendar_id,
                    )
                    .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_import(parent, result, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_import(
                                    parent,
                                    Err("Google Calendar import worker stopped".to_string()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_oauth_task = Some(task);
        cx.notify();
    }

    pub(crate) fn start_google_calendar_scheme_refresh(
        &mut self,
        scheme_id: knotq_model::SchemeId,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress) {
            return;
        }

        let Some(source) = self.workspace.scheme(scheme_id).and_then(|scheme| {
            let SchemeSource::ImportedCalendar(source) = &scheme.source else {
                return None;
            };
            (source.provider == CalendarProvider::Google).then_some(source.clone())
        }) else {
            return;
        };

        let accounts = self
            .settings
            .google_accounts
            .iter()
            .filter(|account| google_account_matches_calendar_source(account, &source))
            .cloned()
            .collect::<Vec<_>>();
        if accounts.is_empty() {
            eprintln!("Google Calendar refresh failed: no stored Google account for this calendar");
            self.google_oauth_status = GoogleOAuthStatus::Error;
            cx.notify();
            return;
        }

        let config = match google_oauth_config_for_existing_accounts(&accounts) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Google Calendar refresh failed: {err:#}");
                self.google_oauth_status = GoogleOAuthStatus::Error;
                cx.notify();
                return;
            }
        };
        let sources = vec![ExistingGoogleCalendarSource {
            account_id: source.account_id,
            account_email: source.account_email,
            calendar_id: source.calendar_id,
            sync_token: source.sync_token,
        }];

        self.google_oauth_status = GoogleOAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = run_google_calendar_background_sync(config, accounts, sources)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_scheme_refresh(result, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_scheme_refresh(
                                    Err("Google Calendar refresh worker stopped".to_string()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_oauth_task = Some(task);
        cx.notify();
    }

    pub(crate) fn start_google_calendar_scheme_reconnect(
        &mut self,
        scheme_id: knotq_model::SchemeId,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress) {
            return;
        }

        let Some(source) = self.workspace.scheme(scheme_id).and_then(|scheme| {
            let SchemeSource::ImportedCalendar(source) = &scheme.source else {
                return None;
            };
            (source.provider == CalendarProvider::Google).then_some(source.clone())
        }) else {
            return;
        };

        let config = match google_oauth_config_from_env() {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Google Calendar reconnect failed: {err:#}");
                self.show_google_calendar_error("Google Calendar reconnect", format!("{err:#}"));
                self.google_oauth_status = GoogleOAuthStatus::Error;
                cx.notify();
                return;
            }
        };

        self.google_oauth_status = GoogleOAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = run_google_calendar_scheme_reconnect(config, source)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_scheme_refresh(result, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_scheme_refresh(
                                    Err("Google Calendar reconnect worker stopped".to_string()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_oauth_task = Some(task);
        cx.notify();
    }

    fn google_calendar_background_snapshot(&self) -> Option<GoogleCalendarBackgroundSnapshot> {
        if self.google_oauth_task.is_some()
            || self.settings.google_accounts.is_empty()
            || matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress)
        {
            return None;
        }
        let sources = google_calendar_sources(&self.workspace);
        if sources.is_empty() {
            return None;
        }
        let config = google_oauth_config_for_existing_accounts(&self.settings.google_accounts)
            .map_err(|err| {
                eprintln!("background Google Calendar sync disabled: {err:#}");
                err
            })
            .ok()?;
        Some(GoogleCalendarBackgroundSnapshot {
            config,
            accounts: self.settings.google_accounts.clone(),
            sources,
        })
    }

    fn finish_google_calendar_import(
        &mut self,
        parent: FolderId,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        cx: &mut Context<Self>,
    ) {
        self.google_oauth_task = None;
        self.finish_google_sync_result(result, parent, true, true, true, "import", cx);
    }

    fn finish_google_calendar_picker_load(
        &mut self,
        parent: FolderId,
        result: std::result::Result<GoogleCalendarPickerLoadResult, String>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .google_calendar_picker
            .as_ref()
            .is_some_and(|picker| picker.parent == parent)
        {
            return;
        }

        self.google_calendar_picker_task = None;
        match result {
            Ok(result) => {
                let accounts_changed = self.upsert_google_accounts(result.accounts);
                if accounts_changed {
                    self.save_app_settings();
                }
                self.google_calendar_picker = Some(GoogleCalendarPickerState {
                    parent,
                    status: GoogleCalendarPickerStatus::Loaded {
                        accounts: result.picker_accounts,
                    },
                });
            }
            Err(err) => {
                eprintln!("Google Calendar selector failed: {err}");
                self.google_calendar_picker = Some(GoogleCalendarPickerState {
                    parent,
                    status: GoogleCalendarPickerStatus::Error(err),
                });
            }
        }
        cx.notify();
    }

    fn finish_google_calendar_scheme_refresh(
        &mut self,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        cx: &mut Context<Self>,
    ) {
        self.google_oauth_task = None;
        self.finish_google_sync_result(
            result,
            self.workspace.root,
            false,
            false,
            true,
            "refresh",
            cx,
        );
    }

    fn finish_google_calendar_background_sync(
        &mut self,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        cx: &mut Context<Self>,
    ) {
        self.finish_google_sync_result(
            result,
            self.workspace.root,
            false,
            false,
            false,
            "background sync",
            cx,
        );
    }

    fn finish_google_sync_result(
        &mut self,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        parent: FolderId,
        create_missing: bool,
        open_first_imported: bool,
        always_notify: bool,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(result) => {
                let accounts_changed = self.upsert_google_accounts(result.accounts);
                let applied = self.apply_imported_google_calendars(
                    parent,
                    result.calendars,
                    create_missing,
                    open_first_imported,
                    cx,
                );
                if accounts_changed || create_missing {
                    self.save_app_settings();
                }
                if !result.failures.is_empty() {
                    let failures = result.failures;
                    for failure in &failures {
                        eprintln!("Google Calendar {label} failed: {failure}");
                    }
                    if always_notify {
                        self.show_google_calendar_error(
                            format!("Google Calendar {label} failed"),
                            failures.join("\n"),
                        );
                        self.google_oauth_status = GoogleOAuthStatus::Error;
                    }
                } else if always_notify {
                    self.google_oauth_status = GoogleOAuthStatus::Idle;
                }
                if applied.content_changed {
                    self.reschedule_notifications();
                }
                if always_notify || applied.content_changed {
                    cx.notify();
                }
            }
            Err(err) => {
                eprintln!("Google Calendar {label} failed: {err}");
                if always_notify {
                    self.show_google_calendar_error(
                        format!("Google Calendar {label} failed"),
                        err.clone(),
                    );
                    self.google_oauth_status = GoogleOAuthStatus::Error;
                    cx.notify();
                }
            }
        }
    }

    fn show_google_calendar_error(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.notice_modal = Some(NoticeModal {
            title: title.into(),
            message: message.into(),
            button_label: "OK".to_string(),
        });
    }

    fn upsert_google_accounts(&mut self, accounts: Vec<GoogleOAuthAccount>) -> bool {
        let mut changed = false;
        for account in accounts {
            if let Some(existing) = self.settings.google_accounts.iter_mut().find(|existing| {
                existing.client_id == account.client_id && existing.account_id == account.account_id
            }) {
                if existing != &account {
                    *existing = account;
                    changed = true;
                }
            } else {
                self.settings.google_accounts.push(account);
                changed = true;
            }
        }
        changed
    }

    fn apply_imported_google_calendars(
        &mut self,
        parent: FolderId,
        calendars: Vec<ImportedGoogleCalendar>,
        create_missing: bool,
        open_first_imported: bool,
        cx: &mut Context<Self>,
    ) -> GoogleCalendarApplyResult {
        let parent = if self.workspace.folder(parent).is_some() {
            parent
        } else {
            self.workspace.root
        };
        if parent != self.workspace.root {
            if let Some(folder) = self.workspace.folders.get_mut(&parent) {
                folder.expanded = true;
            }
        }

        let mut first_scheme = None;
        let mut synced = 0usize;
        let mut content_changed = false;
        for calendar in calendars {
            let existing_scheme_ids = active_google_calendar_scheme_ids(&self.workspace, &calendar);
            let existing_scheme_id = existing_scheme_ids
                .first()
                .copied()
                .or_else(|| find_google_calendar_scheme(&self.workspace, &calendar));
            if self.delete_duplicate_google_calendar_schemes(
                existing_scheme_ids.get(1..).unwrap_or(&[]),
                cx,
            ) {
                content_changed = true;
            }
            let scheme_id = match existing_scheme_id {
                Some(scheme_id) => {
                    if create_missing && self.workspace.is_scheme_deleted(scheme_id) {
                        self.restore_deleted_scheme(scheme_id, cx);
                        content_changed = true;
                    }
                    scheme_id
                }
                None if create_missing => {
                    let color_index = calendar.color_index;
                    let mut scheme = Scheme::new(calendar.name.clone(), color_index);
                    let id = scheme.id;
                    scheme.source = google_calendar_source(&calendar);
                    self.workspace.schemes.insert(id, scheme);
                    if let Some(folder) = self.workspace.folders.get_mut(&parent) {
                        folder.children.push(NodeRef::Scheme(id));
                    }
                    content_changed = true;
                    id
                }
                None => continue,
            };

            if let Some(scheme) = self.workspace.schemes.get_mut(&scheme_id) {
                let should_update_name = existing_scheme_id.is_none();
                let metadata_changed =
                    apply_google_calendar_metadata(scheme, &calendar, should_update_name);
                let items_changed = apply_google_calendar_items(scheme, &calendar);
                self.state.mark_scheme_dirty(scheme_id);
                let scheme_content_changed = metadata_changed || items_changed;
                if scheme_content_changed {
                    content_changed = true;
                    if self
                        .scheme_editor
                        .as_ref()
                        .is_some_and(|(id, _)| *id == scheme_id)
                    {
                        self.scheme_editor = None;
                        self._editor_subscription = None;
                    }
                    self.scheme_sessions.remove(&scheme_id);
                }
                first_scheme.get_or_insert(scheme_id);
                synced += 1;
            }
        }

        if synced > 0 {
            self.reconcile_workspace_ui_state();
            self.state.mark_index_dirty();
            if open_first_imported {
                if let Some(scheme_id) = first_scheme {
                    self.open_scheme(scheme_id, None);
                }
            }
        }
        GoogleCalendarApplyResult { content_changed }
    }

    fn delete_duplicate_google_calendar_schemes(
        &mut self,
        scheme_ids: &[SchemeId],
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;
        for scheme_id in scheme_ids.iter().copied() {
            if self.workspace.is_scheme_deleted(scheme_id)
                || !self.workspace.schemes.contains_key(&scheme_id)
            {
                continue;
            }

            let was_selected = self.selection.scheme_id == Some(scheme_id);
            let fallback = was_selected
                .then(|| self.first_visible_scheme_id_except(scheme_id))
                .flatten();

            let mut origin = None;
            for (folder_id, folder) in self.workspace.folders.iter_mut() {
                let mut index = 0usize;
                while index < folder.children.len() {
                    if folder.children[index] == NodeRef::Scheme(scheme_id) {
                        if origin.is_none() {
                            origin = Some((*folder_id, index));
                        }
                        folder.children.remove(index);
                    } else {
                        index += 1;
                    }
                }
            }

            if let Some((folder_id, position)) = origin {
                self.workspace
                    .mark_scheme_deleted_from(scheme_id, folder_id, position);
            } else {
                self.workspace.mark_scheme_deleted(scheme_id);
            }
            self.trash_expanded = true;
            self.state.mark_index_dirty();
            if self
                .scheme_editor
                .as_ref()
                .is_some_and(|(id, _)| *id == scheme_id)
            {
                self.scheme_editor = None;
                self._editor_subscription = None;
            }
            self.close_popovers_for_scheme(scheme_id);
            self.scheme_sessions.remove(&scheme_id);
            if was_selected {
                if let Some(next_id) = fallback {
                    self.open_scheme(next_id, None);
                } else {
                    self.open_union();
                    self.selection.scheme_id = None;
                    self.selection.focused_item_id = None;
                }
                cx.notify();
            }
            changed = true;
        }
        changed
    }
}

fn google_oauth_config_from_env() -> Result<GoogleOAuthConfig> {
    let client_id = env::var("KNOTQ_GOOGLE_CLIENT_ID")
        .or_else(|_| env::var("GOOGLE_CLIENT_ID"))
        .context("set KNOTQ_GOOGLE_CLIENT_ID to a Google Desktop OAuth client id")?;
    let client_secret = env::var("KNOTQ_GOOGLE_CLIENT_SECRET")
        .or_else(|_| env::var("GOOGLE_CLIENT_SECRET"))
        .ok()
        .filter(|secret| !secret.trim().is_empty());
    Ok(GoogleOAuthConfig {
        client_id,
        client_secret,
    })
}

fn google_oauth_config_for_existing_accounts(
    accounts: &[GoogleOAuthAccount],
) -> Result<GoogleOAuthConfig> {
    let client_id = env::var("KNOTQ_GOOGLE_CLIENT_ID")
        .or_else(|_| env::var("GOOGLE_CLIENT_ID"))
        .ok()
        .or_else(|| accounts.first().map(|account| account.client_id.clone()))
        .context("no stored Google account is available")?;
    let client_secret = env::var("KNOTQ_GOOGLE_CLIENT_SECRET")
        .or_else(|_| env::var("GOOGLE_CLIENT_SECRET"))
        .ok()
        .filter(|secret| !secret.trim().is_empty());
    Ok(GoogleOAuthConfig {
        client_id,
        client_secret,
    })
}

fn run_google_calendar_picker_load(
    config: GoogleOAuthConfig,
    accounts: Vec<GoogleOAuthAccount>,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
) -> Result<GoogleCalendarPickerLoadResult> {
    let mut updated_accounts = Vec::new();
    let mut picker_accounts = Vec::new();

    for mut account in accounts {
        let label = account
            .email
            .clone()
            .filter(|email| !email.trim().is_empty())
            .unwrap_or_else(|| account.account_id.clone());

        if let Err(err) = refresh_google_access_token_if_needed(&config, &mut account) {
            picker_accounts.push(GoogleCalendarPickerAccount {
                account_id: account.account_id.clone(),
                label,
                calendars: Vec::new(),
                error: Some(format!("{err:#}")),
            });
            updated_accounts.push(account);
            continue;
        }

        match list_google_calendars(&account.access_token) {
            Ok(calendars) => {
                let calendars = calendars
                    .into_iter()
                    .map(|calendar| {
                        let already_added = existing_sources.iter().any(|source| {
                            source.calendar_id == calendar.id
                                && existing_source_matches_google_account(source, &account)
                        });
                        GoogleCalendarPickerCalendar {
                            id: calendar.id.clone(),
                            label: google_calendar_name(&calendar),
                            already_added,
                        }
                    })
                    .collect();
                picker_accounts.push(GoogleCalendarPickerAccount {
                    account_id: account.account_id.clone(),
                    label,
                    calendars,
                    error: None,
                });
            }
            Err(err) => {
                picker_accounts.push(GoogleCalendarPickerAccount {
                    account_id: account.account_id.clone(),
                    label,
                    calendars: Vec::new(),
                    error: Some(format!("{err:#}")),
                });
            }
        }
        updated_accounts.push(account);
    }

    Ok(GoogleCalendarPickerLoadResult {
        accounts: updated_accounts,
        picker_accounts,
    })
}

fn run_google_calendar_import(
    config: GoogleOAuthConfig,
    _existing_accounts: Vec<GoogleOAuthAccount>,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
) -> Result<GoogleCalendarImportResult> {
    let accounts = vec![run_google_oauth(config.clone())?];
    run_google_calendar_sync(
        config,
        accounts,
        existing_sources,
        GoogleCalendarImportMode::MissingOnly,
        None,
    )
}

fn run_google_calendar_import_existing_account_calendar(
    config: GoogleOAuthConfig,
    account: GoogleOAuthAccount,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
    calendar_id: String,
) -> Result<GoogleCalendarImportResult> {
    run_google_calendar_sync(
        config,
        vec![account],
        existing_sources,
        GoogleCalendarImportMode::MissingOnly,
        Some(calendar_id),
    )
}

fn run_google_calendar_scheme_reconnect(
    config: GoogleOAuthConfig,
    source: ImportedCalendarSource,
) -> Result<GoogleCalendarImportResult> {
    let account = run_google_oauth(config.clone())?;
    if !google_account_matches_calendar_source(&account, &source) {
        let signed_in = account
            .email
            .clone()
            .unwrap_or_else(|| account.account_id.clone());
        bail!(
            "signed in as {signed_in}, but this calendar belongs to {}",
            google_calendar_source_target_label(&source)
        );
    }
    run_google_calendar_sync(
        config,
        vec![account],
        vec![ExistingGoogleCalendarSource {
            account_id: source.account_id,
            account_email: source.account_email,
            calendar_id: source.calendar_id,
            sync_token: source.sync_token,
        }],
        GoogleCalendarImportMode::ExistingOnly,
        None,
    )
}

fn run_google_calendar_background_sync(
    config: GoogleOAuthConfig,
    existing_accounts: Vec<GoogleOAuthAccount>,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
) -> Result<GoogleCalendarImportResult> {
    if existing_accounts.is_empty() || existing_sources.is_empty() {
        return Ok(GoogleCalendarImportResult {
            accounts: Vec::new(),
            calendars: Vec::new(),
            failures: Vec::new(),
        });
    }
    run_google_calendar_sync(
        config,
        existing_accounts,
        existing_sources,
        GoogleCalendarImportMode::ExistingOnly,
        None,
    )
}

fn run_google_calendar_sync(
    config: GoogleOAuthConfig,
    accounts: Vec<GoogleOAuthAccount>,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
    mode: GoogleCalendarImportMode,
    target_calendar_id: Option<String>,
) -> Result<GoogleCalendarImportResult> {
    let mut updated_accounts = Vec::new();
    let mut calendars = Vec::new();
    let mut failures = Vec::new();

    for mut account in accounts {
        if let Err(err) = refresh_google_access_token_if_needed(&config, &mut account) {
            failures.push(format!(
                "{}: {err:#}",
                account.email.as_deref().unwrap_or(&account.account_id)
            ));
            continue;
        }

        match import_google_account_calendars(
            &account,
            &existing_sources,
            mode,
            target_calendar_id.as_deref(),
        ) {
            Ok((mut imported, mut account_failures)) => {
                calendars.append(&mut imported);
                failures.append(&mut account_failures);
            }
            Err(err) => failures.push(format!(
                "{}: {err:#}",
                account.email.as_deref().unwrap_or(&account.account_id)
            )),
        }
        updated_accounts.push(account);
    }

    Ok(GoogleCalendarImportResult {
        accounts: updated_accounts,
        calendars,
        failures,
    })
}

fn run_google_oauth(config: GoogleOAuthConfig) -> Result<GoogleOAuthAccount> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind OAuth loopback listener")?;
    listener
        .set_nonblocking(true)
        .context("make OAuth loopback listener nonblocking")?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let state = random_token(32);
    let code_verifier = random_token(96);
    let code_challenge = code_challenge(&code_verifier);
    let scope = GOOGLE_OAUTH_SCOPES.join(" ");
    let auth_url = google_auth_url(
        &config.client_id,
        &redirect_uri,
        &scope,
        &state,
        &code_challenge,
    );

    open_browser(&auth_url)?;
    let code = wait_for_oauth_code(&listener, &state, StdDuration::from_secs(120))?;
    let token = exchange_auth_code(&config, &redirect_uri, &code, &code_verifier)?;
    let refresh_token = token
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("Google did not return a refresh token"))?;
    let claims = token.id_token.as_deref().and_then(decode_id_token_claims);
    let account_id = claims
        .as_ref()
        .and_then(|claims| claims.sub.clone())
        .or_else(|| claims.as_ref().and_then(|claims| claims.email.clone()))
        .unwrap_or_else(|| "google".to_string());
    let expires_at = token
        .expires_in
        .map(|seconds| Utc::now() + Duration::seconds(seconds));

    Ok(GoogleOAuthAccount {
        account_id,
        email: claims.and_then(|claims| claims.email),
        client_id: config.client_id,
        access_token: token.access_token,
        refresh_token,
        expires_at,
        scope: token.scope.unwrap_or(scope),
    })
}

fn google_auth_url(
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    let params = [
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", scope),
        ("state", state),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{GOOGLE_AUTH_URL}?{query}")
}

fn wait_for_oauth_code(
    listener: &TcpListener,
    expected_state: &str,
    timeout: StdDuration,
) -> Result<String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let result = read_oauth_callback(&mut stream, expected_state);
                let body = if result.is_ok() {
                    "Google Calendar is connected. You can close this tab and return to KnotQ."
                } else {
                    "Google Calendar connection failed. You can close this tab and return to KnotQ."
                };
                let _ = write_http_response(&mut stream, body);
                return result;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(StdDuration::from_millis(100));
            }
            Err(err) => return Err(err).context("accept OAuth callback"),
        }
    }
    bail!("Google OAuth timed out waiting for browser callback")
}

fn read_oauth_callback(stream: &mut TcpStream, expected_state: &str) -> Result<String> {
    let mut buffer = [0u8; 4096];
    let len = stream.read(&mut buffer).context("read OAuth callback")?;
    let request = String::from_utf8_lossy(&buffer[..len]);
    let request_target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("invalid OAuth callback request"))?;
    let params = query_params(request_target)?;
    if params.get("state").map(String::as_str) != Some(expected_state) {
        bail!("Google OAuth returned an unexpected state");
    }
    if let Some(error) = params.get("error") {
        bail!("Google OAuth error: {error}");
    }
    params
        .get("code")
        .cloned()
        .ok_or_else(|| anyhow!("Google OAuth callback did not include a code"))
}

fn query_params(request_target: &str) -> Result<HashMap<String, String>> {
    let query = request_target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or("");
    let query = query.split('#').next().unwrap_or(query);
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = urlencoding::decode(key)?.into_owned();
        let value = urlencoding::decode(value)?.into_owned();
        params.insert(key, value);
    }
    Ok(params)
}

fn exchange_auth_code(
    config: &GoogleOAuthConfig,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
) -> Result<GoogleTokenResponse> {
    let mut form = vec![
        ("client_id", config.client_id.as_str()),
        ("code", code),
        ("code_verifier", code_verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret.as_str()));
    }

    ureq::post(GOOGLE_TOKEN_URL)
        .send_form(&form)
        .map_err(google_http_error)?
        .into_json::<GoogleTokenResponse>()
        .context("parse Google OAuth token response")
}

fn refresh_google_access_token_if_needed(
    config: &GoogleOAuthConfig,
    account: &mut GoogleOAuthAccount,
) -> Result<()> {
    let still_valid = account
        .expires_at
        .is_some_and(|expires_at| expires_at > Utc::now() + Duration::seconds(60));
    if still_valid {
        return Ok(());
    }

    let mut form = vec![
        ("client_id", account.client_id.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", account.refresh_token.as_str()),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret.as_str()));
    }

    let token = ureq::post(GOOGLE_TOKEN_URL)
        .send_form(&form)
        .map_err(google_http_error)?
        .into_json::<GoogleTokenResponse>()
        .context("parse Google OAuth refresh response")?;

    account.access_token = token.access_token;
    account.expires_at = token
        .expires_in
        .map(|seconds| Utc::now() + Duration::seconds(seconds));
    if let Some(scope) = token.scope {
        account.scope = scope;
    }
    Ok(())
}

fn import_google_account_calendars(
    account: &GoogleOAuthAccount,
    existing_sources: &[ExistingGoogleCalendarSource],
    mode: GoogleCalendarImportMode,
    target_calendar_id: Option<&str>,
) -> Result<(Vec<ImportedGoogleCalendar>, Vec<String>)> {
    let calendars = list_google_calendars(&account.access_token)?;
    let fallback_count = calendars.len().max(1);
    let mut imported = Vec::new();
    let mut failures = Vec::new();

    for (index, calendar) in calendars.into_iter().enumerate() {
        if target_calendar_id.is_some_and(|target| target != calendar.id) {
            continue;
        }
        let existing = existing_sources.iter().find(|source| {
            source.calendar_id == calendar.id
                && existing_source_matches_google_account(source, account)
        });
        match mode {
            GoogleCalendarImportMode::ExistingOnly if existing.is_none() => continue,
            GoogleCalendarImportMode::MissingOnly if existing.is_some() => continue,
            _ => {}
        }
        let sync_token = existing.and_then(|source| source.sync_token.clone());
        let events = match list_google_events(&account.access_token, &calendar.id, sync_token) {
            Ok(events) => events,
            Err(err) => {
                failures.push(format!("{}: {err}", google_calendar_name(&calendar)));
                continue;
            }
        };

        let mut items = events
            .events
            .iter()
            .filter_map(|event| google_event_to_item(account, &calendar.id, event))
            .collect::<Vec<_>>();
        sort_imported_items(&mut items);

        let deleted = events
            .events
            .iter()
            .filter(|event| event.status.as_deref() == Some("cancelled"))
            .map(google_event_key)
            .collect();

        imported.push(ImportedGoogleCalendar {
            account_id: account.account_id.clone(),
            account_email: account.email.clone(),
            calendar_id: calendar.id.clone(),
            name: IMPORTED_GOOGLE_CALENDAR_SCHEME_NAME.to_string(),
            color_index: google_calendar_color_index(
                calendar.background_color.as_deref(),
                index % fallback_count,
            ),
            sync_token: events.sync_token,
            full_sync: events.full_sync,
            items,
            deleted,
        });
    }

    Ok((imported, failures))
}

struct GoogleEventsSync {
    events: Vec<GoogleEvent>,
    sync_token: Option<String>,
    full_sync: bool,
}

fn list_google_calendars(access_token: &str) -> Result<Vec<GoogleCalendarListEntry>> {
    let mut page_token: Option<String> = None;
    let mut calendars = Vec::new();

    loop {
        let mut params = vec![
            ("maxResults", "250".to_string()),
            ("minAccessRole", "reader".to_string()),
        ];
        if let Some(token) = &page_token {
            params.push(("pageToken", token.clone()));
        }
        let url = with_query(GOOGLE_CALENDAR_LIST_URL, &params);
        let response: GoogleCalendarListResponse = google_get_json(&url, access_token)?;
        calendars.extend(
            response
                .items
                .into_iter()
                .filter(|calendar| calendar.deleted != Some(true) && calendar.hidden != Some(true)),
        );
        page_token = response.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    let visible = calendars
        .iter()
        .filter(|calendar| calendar.selected != Some(false) || calendar.primary == Some(true))
        .cloned()
        .collect::<Vec<_>>();
    if visible.is_empty() {
        Ok(calendars)
    } else {
        Ok(visible)
    }
}

fn list_google_events(
    access_token: &str,
    calendar_id: &str,
    sync_token: Option<String>,
) -> Result<GoogleEventsSync> {
    match list_google_events_once(access_token, calendar_id, sync_token.clone()) {
        Ok(events) => Ok(events),
        Err(err) if err.status == Some(410) && sync_token.is_some() => {
            Ok(list_google_events_once(access_token, calendar_id, None)?)
        }
        Err(err) => Err(anyhow!(err)),
    }
}

fn list_google_events_once(
    access_token: &str,
    calendar_id: &str,
    sync_token: Option<String>,
) -> std::result::Result<GoogleEventsSync, GoogleApiError> {
    let base = format!(
        "{GOOGLE_EVENTS_BASE_URL}/{}/events",
        urlencoding::encode(calendar_id)
    );
    let mut page_token: Option<String> = None;
    let full_sync = sync_token.is_none();
    let mut events = Vec::new();
    let mut next_sync_token = sync_token.clone();

    loop {
        let mut params = vec![
            ("maxResults", "2500".to_string()),
            ("singleEvents", "false".to_string()),
        ];
        if let Some(token) = &sync_token {
            params.push(("syncToken", token.clone()));
            params.push(("showDeleted", "true".to_string()));
        } else {
            params.push(("showDeleted", "false".to_string()));
        }
        if let Some(token) = &page_token {
            params.push(("pageToken", token.clone()));
        }

        let url = with_query(&base, &params);
        let response: GoogleEventsResponse = google_get_json(&url, access_token)?;
        events.extend(response.items);
        if let Some(token) = response.next_sync_token {
            next_sync_token = Some(token);
        }
        page_token = response.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    Ok(GoogleEventsSync {
        events,
        sync_token: next_sync_token,
        full_sync,
    })
}

fn google_get_json<T: DeserializeOwned>(
    url: &str,
    access_token: &str,
) -> std::result::Result<T, GoogleApiError> {
    let auth = format!("Bearer {access_token}");
    let response = ureq::get(url)
        .set("Authorization", &auth)
        .call()
        .map_err(google_api_error)?;
    response.into_json::<T>().map_err(|err| GoogleApiError {
        status: None,
        message: format!("parse Google Calendar response: {err}"),
    })
}

fn google_api_error(err: ureq::Error) -> GoogleApiError {
    match err {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            GoogleApiError {
                status: Some(status),
                message: format!("Google Calendar HTTP {status}: {body}"),
            }
        }
        ureq::Error::Transport(err) => GoogleApiError {
            status: None,
            message: format!("Google Calendar request failed: {err}"),
        },
    }
}

fn google_calendar_sources(workspace: &Workspace) -> Vec<ExistingGoogleCalendarSource> {
    google_calendar_sources_matching(workspace, |_| true)
}

fn active_google_calendar_sources(workspace: &Workspace) -> Vec<ExistingGoogleCalendarSource> {
    google_calendar_sources_matching(workspace, |scheme_id| {
        !workspace.is_scheme_deleted(scheme_id)
    })
}

fn google_calendar_sources_matching(
    workspace: &Workspace,
    include_scheme: impl Fn(knotq_model::SchemeId) -> bool,
) -> Vec<ExistingGoogleCalendarSource> {
    workspace
        .schemes
        .iter()
        .filter_map(|(scheme_id, scheme)| {
            if !include_scheme(*scheme_id) {
                return None;
            }
            let SchemeSource::ImportedCalendar(source) = &scheme.source else {
                return None;
            };
            if source.provider != CalendarProvider::Google {
                return None;
            }
            Some(ExistingGoogleCalendarSource {
                account_id: source.account_id.clone(),
                account_email: source.account_email.clone(),
                calendar_id: source.calendar_id.clone(),
                sync_token: source.sync_token.clone(),
            })
        })
        .collect()
}

fn existing_source_matches_google_account(
    source: &ExistingGoogleCalendarSource,
    account: &GoogleOAuthAccount,
) -> bool {
    if source.account_id == account.account_id {
        return true;
    }
    let Some(account_email) = account.email.as_deref() else {
        return false;
    };
    let source_email = source.account_email.as_deref().or_else(|| {
        source
            .account_id
            .contains('@')
            .then_some(source.account_id.as_str())
    });
    source_email.is_some_and(|source_email| emails_match(account_email, source_email))
}

fn find_google_calendar_scheme(
    workspace: &Workspace,
    calendar: &ImportedGoogleCalendar,
) -> Option<knotq_model::SchemeId> {
    workspace.schemes.values().find_map(|scheme| {
        let SchemeSource::ImportedCalendar(source) = &scheme.source else {
            return None;
        };
        (source.provider == CalendarProvider::Google
            && source.calendar_id == calendar.calendar_id
            && google_import_matches_source(calendar, source))
        .then_some(scheme.id)
    })
}

fn active_google_calendar_scheme_ids(
    workspace: &Workspace,
    calendar: &ImportedGoogleCalendar,
) -> Vec<SchemeId> {
    let mut ids = Vec::new();
    let mut seen_folders = HashSet::new();
    let mut seen_schemes = HashSet::new();
    collect_google_calendar_scheme_ids(
        workspace,
        workspace.root,
        calendar,
        &mut seen_folders,
        &mut seen_schemes,
        &mut ids,
    );

    let mut unreferenced = workspace
        .schemes
        .keys()
        .copied()
        .filter(|id| !seen_schemes.contains(id))
        .filter(|id| google_calendar_scheme_matches(workspace, *id, calendar))
        .collect::<Vec<_>>();
    unreferenced.sort_by_key(|id| id.to_string());
    ids.extend(unreferenced);
    ids
}

fn collect_google_calendar_scheme_ids(
    workspace: &Workspace,
    folder_id: FolderId,
    calendar: &ImportedGoogleCalendar,
    seen_folders: &mut HashSet<FolderId>,
    seen_schemes: &mut HashSet<SchemeId>,
    out: &mut Vec<SchemeId>,
) {
    if !seen_folders.insert(folder_id) {
        return;
    }
    let Some(folder) = workspace.folders.get(&folder_id) else {
        return;
    };
    for child in &folder.children {
        match *child {
            NodeRef::Scheme(id) => {
                seen_schemes.insert(id);
                if google_calendar_scheme_matches(workspace, id, calendar) {
                    out.push(id);
                }
            }
            NodeRef::Folder(id) => collect_google_calendar_scheme_ids(
                workspace,
                id,
                calendar,
                seen_folders,
                seen_schemes,
                out,
            ),
        }
    }
}

fn google_calendar_scheme_matches(
    workspace: &Workspace,
    scheme_id: SchemeId,
    calendar: &ImportedGoogleCalendar,
) -> bool {
    if workspace.is_scheme_deleted(scheme_id) {
        return false;
    }
    let Some(scheme) = workspace.schemes.get(&scheme_id) else {
        return false;
    };
    let SchemeSource::ImportedCalendar(source) = &scheme.source else {
        return false;
    };
    source.provider == CalendarProvider::Google
        && source.calendar_id == calendar.calendar_id
        && google_import_matches_source(calendar, source)
}

fn find_google_calendar_scheme_for_account(
    workspace: &Workspace,
    account: &GoogleOAuthAccount,
    calendar_id: &str,
) -> Option<knotq_model::SchemeId> {
    find_google_calendar_scheme_for_account_matching(workspace, account, calendar_id, |deleted| {
        !deleted
    })
}

fn find_archived_google_calendar_scheme_for_account(
    workspace: &Workspace,
    account: &GoogleOAuthAccount,
    calendar_id: &str,
) -> Option<knotq_model::SchemeId> {
    find_google_calendar_scheme_for_account_matching(workspace, account, calendar_id, |deleted| {
        deleted
    })
}

fn find_google_calendar_scheme_for_account_matching(
    workspace: &Workspace,
    account: &GoogleOAuthAccount,
    calendar_id: &str,
    include_deleted_state: impl Fn(bool) -> bool,
) -> Option<knotq_model::SchemeId> {
    workspace.schemes.values().find_map(|scheme| {
        let deleted = workspace.is_scheme_deleted(scheme.id);
        if !include_deleted_state(deleted) {
            return None;
        }
        let SchemeSource::ImportedCalendar(source) = &scheme.source else {
            return None;
        };
        (source.provider == CalendarProvider::Google
            && source.calendar_id == calendar_id
            && google_account_matches_calendar_source(account, source))
        .then_some(scheme.id)
    })
}

fn google_import_matches_source(
    calendar: &ImportedGoogleCalendar,
    source: &ImportedCalendarSource,
) -> bool {
    if source.account_id == calendar.account_id {
        return true;
    }
    let Some(account_email) = calendar.account_email.as_deref() else {
        return false;
    };
    let source_email = source.account_email.as_deref().or_else(|| {
        source
            .account_id
            .contains('@')
            .then_some(source.account_id.as_str())
    });
    source_email.is_some_and(|source_email| emails_match(account_email, source_email))
}

fn google_calendar_source(calendar: &ImportedGoogleCalendar) -> SchemeSource {
    SchemeSource::ImportedCalendar(ImportedCalendarSource {
        provider: CalendarProvider::Google,
        account_id: calendar.account_id.clone(),
        account_email: calendar.account_email.clone(),
        calendar_id: calendar.calendar_id.clone(),
        sync_token: calendar.sync_token.clone(),
        read_only: true,
        last_synced_at: Some(Utc::now()),
    })
}

fn apply_google_calendar_metadata(
    scheme: &mut Scheme,
    calendar: &ImportedGoogleCalendar,
    should_update_name: bool,
) -> bool {
    let next_source = google_calendar_source(calendar);
    let metadata_changed =
        (should_update_name && scheme.name != calendar.name) || scheme.source != next_source;
    if should_update_name {
        scheme.name = calendar.name.clone();
    }
    scheme.source = next_source;
    metadata_changed
}

fn apply_google_calendar_items(scheme: &mut Scheme, calendar: &ImportedGoogleCalendar) -> bool {
    if calendar.full_sync {
        let mut items = calendar
            .items
            .iter()
            .cloned()
            .map(|item| {
                let existing = item
                    .external
                    .as_ref()
                    .and_then(|external| find_existing_external_item(&scheme.items, external));
                merge_imported_item(existing, item)
            })
            .collect::<Vec<_>>();
        sort_imported_items(&mut items);
        if imported_item_lists_equal(&scheme.items, &items) {
            return false;
        }
        scheme.items = items;
        return true;
    }

    let mut changed = false;
    scheme.items.retain(|item| {
        let Some(external) = &item.external else {
            return true;
        };
        if external.provider != CalendarProvider::Google
            || external.account_id != calendar.account_id
            || external.calendar_id != calendar.calendar_id
        {
            return true;
        }
        let keep = !calendar
            .deleted
            .iter()
            .any(|key| external_matches_key(external, key));
        if !keep {
            changed = true;
        }
        keep
    });

    for item in calendar.items.iter().cloned() {
        let Some(external) = item.external.as_ref() else {
            continue;
        };
        if let Some(existing) = scheme.items.iter_mut().find(|candidate| {
            candidate
                .external
                .as_ref()
                .is_some_and(|candidate| external_same_event(candidate, external))
        }) {
            let updated = merge_imported_item(Some(existing), item);
            if !item_content_eq_ignoring_id(existing, &updated) {
                *existing = updated;
                changed = true;
            }
        } else {
            scheme.items.push(item);
            changed = true;
        }
    }
    if changed {
        sort_imported_items(&mut scheme.items);
    }
    changed
}

fn find_existing_external_item<'a>(
    items: &'a [Item],
    external: &ExternalItemSource,
) -> Option<&'a Item> {
    items.iter().find(|candidate| {
        candidate
            .external
            .as_ref()
            .is_some_and(|candidate| external_same_event(candidate, external))
    })
}

fn merge_imported_item(existing: Option<&Item>, mut imported: Item) -> Item {
    let Some(existing) = existing else {
        return imported;
    };
    imported.id = existing.id;
    if item_occurrence_identity_eq(existing, &imported) {
        imported.state = existing.state.clone();
    }
    imported
}

fn imported_item_lists_equal(existing: &[Item], imported: &[Item]) -> bool {
    existing.len() == imported.len()
        && existing
            .iter()
            .zip(imported)
            .all(|(left, right)| item_content_eq_ignoring_id(left, right))
}

fn item_content_eq_ignoring_id(left: &Item, right: &Item) -> bool {
    left.text == right.text
        && left.media == right.media
        && left.marker == right.marker
        && left.indent == right.indent
        && left.start == right.start
        && left.end == right.end
        && left.available == right.available
        && left.repeats == right.repeats
        && left.state == right.state
        && left.priority == right.priority
        && left.external == right.external
}

fn item_occurrence_identity_eq(left: &Item, right: &Item) -> bool {
    left.marker == right.marker
        && left.start == right.start
        && left.end == right.end
        && left.available == right.available
        && left.repeats == right.repeats
}

fn external_matches_key(external: &ExternalItemSource, key: &GoogleExternalEventKey) -> bool {
    external.event_id == key.event_id && external.instance_id == key.instance_id
}

fn external_same_event(left: &ExternalItemSource, right: &ExternalItemSource) -> bool {
    left.provider == right.provider
        && left.account_id == right.account_id
        && left.calendar_id == right.calendar_id
        && left.event_id == right.event_id
        && left.instance_id == right.instance_id
}

fn google_event_to_item(
    account: &GoogleOAuthAccount,
    calendar_id: &str,
    event: &GoogleEvent,
) -> Option<Item> {
    if event.status.as_deref() == Some("cancelled") {
        return None;
    }
    let start = event.start.as_ref().and_then(google_event_datetime_to_utc);
    let end = event.end.as_ref().and_then(google_event_datetime_to_utc);
    if start.is_none() && end.is_none() {
        return None;
    }

    let mut item = Item::new(
        event
            .summary
            .as_deref()
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or("(untitled)"),
    );
    item.marker = ItemMarker::Checkbox;
    item.start = start;
    item.end = end;
    item.repeats = google_event_recurrence(event);
    let key = google_event_key(event);
    item.external = Some(ExternalItemSource {
        provider: CalendarProvider::Google,
        account_id: account.account_id.clone(),
        calendar_id: calendar_id.to_string(),
        event_id: key.event_id,
        instance_id: key.instance_id,
        updated_at: event.updated,
    });
    Some(item)
}

fn google_event_key(event: &GoogleEvent) -> GoogleExternalEventKey {
    GoogleExternalEventKey {
        event_id: event
            .recurring_event_id
            .clone()
            .unwrap_or_else(|| event.id.clone()),
        instance_id: event.recurring_event_id.as_ref().map(|_| event.id.clone()),
    }
}

fn google_event_datetime_to_utc(datetime: &GoogleEventDateTime) -> Option<DateTime<Utc>> {
    datetime
        .date_time
        .or_else(|| datetime.date.and_then(local_date_midnight_utc))
}

fn local_date_midnight_utc(date: NaiveDate) -> Option<DateTime<Utc>> {
    let local = date.and_hms_opt(0, 0, 0)?;
    Local
        .from_local_datetime(&local)
        .earliest()
        .map(|datetime| datetime.with_timezone(&Utc))
}

fn google_event_recurrence(event: &GoogleEvent) -> Option<knotq_model::CalendarRecurrence> {
    let mut recurrence = knotq_model::CalendarRecurrence::default();
    for raw in event.recurrence.as_ref()? {
        let Some((kind, value)) = raw.split_once(':') else {
            continue;
        };
        match kind.to_ascii_uppercase().as_str() {
            "RRULE" => recurrence.rrules.push(value.to_string()),
            "RDATE" => {
                recurrence
                    .rdates
                    .extend(parse_google_calendar_date_times(value));
            }
            "EXDATE" => {
                recurrence
                    .exdates
                    .extend(parse_google_calendar_date_times(value));
            }
            _ => {}
        }
    }
    if recurrence.rrules.is_empty() && recurrence.rdates.is_empty() && recurrence.exdates.is_empty()
    {
        None
    } else {
        recurrence.raw_import =
            serde_json::to_string(event)
                .ok()
                .map(|data| knotq_model::RawCalendarPayload {
                    content_type: "application/vnd.google.calendar.event+json".to_string(),
                    data,
                });
        Some(recurrence)
    }
}

fn parse_google_calendar_date_times(raw: &str) -> Vec<CalendarDateTime> {
    raw.split(',')
        .filter_map(|part| {
            let value = part
                .split_once(':')
                .map(|(_, value)| value)
                .unwrap_or(part)
                .trim();
            DateTime::parse_from_rfc3339(value)
                .map(|datetime| CalendarDateTime::utc(datetime.with_timezone(&Utc)))
                .ok()
                .or_else(|| {
                    chrono::NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
                        .ok()
                        .map(|datetime| {
                            CalendarDateTime::utc(DateTime::from_naive_utc_and_offset(
                                datetime, Utc,
                            ))
                        })
                })
                .or_else(|| {
                    NaiveDate::parse_from_str(value, "%Y%m%d")
                        .ok()
                        .map(|date| CalendarDateTime::Date { date })
                })
        })
        .collect()
}

fn sort_imported_items(items: &mut [Item]) {
    items.sort_by(|left, right| {
        let left_date = left.start.or(left.end);
        let right_date = right.start.or(right.end);
        left_date
            .cmp(&right_date)
            .then_with(|| left.text.cmp(&right.text))
            .then_with(|| left.id.0.cmp(&right.id.0))
    });
}

fn google_calendar_name(calendar: &GoogleCalendarListEntry) -> String {
    calendar
        .summary_override
        .as_deref()
        .or(calendar.summary.as_deref())
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or("Google Calendar")
        .to_string()
}

fn google_calendar_color_index(background: Option<&str>, fallback: usize) -> u8 {
    let Some(rgb) = background.and_then(parse_google_hex_color) else {
        return (fallback % crate::theme_gpui::PALETTE.len()) as u8;
    };

    crate::theme_gpui::PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, palette)| rgb_distance(rgb, **palette))
        .map(|(idx, _)| idx as u8)
        .unwrap_or((fallback % crate::theme_gpui::PALETTE.len()) as u8)
}

fn parse_google_hex_color(raw: &str) -> Option<u32> {
    let value = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    if value.len() != 6 {
        return None;
    }
    u32::from_str_radix(value, 16).ok()
}

fn rgb_distance(left: u32, right: u32) -> u32 {
    let channels = |rgb: u32| {
        (
            ((rgb >> 16) & 0xff) as i32,
            ((rgb >> 8) & 0xff) as i32,
            (rgb & 0xff) as i32,
        )
    };
    let (lr, lg, lb) = channels(left);
    let (rr, rg, rb) = channels(right);
    ((lr - rr).pow(2) + (lg - rg).pow(2) + (lb - rb).pow(2)) as u32
}

fn with_query(base: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return base.to_string();
    }
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

fn google_http_error(err: ureq::Error) -> anyhow::Error {
    match err {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            anyhow!("Google OAuth HTTP {status}: {body}")
        }
        ureq::Error::Transport(err) => anyhow!("Google OAuth request failed: {err}"),
    }
}

fn write_http_response(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>KnotQ</title></head><body>{body}</body></html>"
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    )
}

pub(crate) fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command.spawn().context("open URL in browser")?;
    Ok(())
}

fn random_token(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), len)
}

fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn decode_id_token_claims(id_token: &str) -> Option<GoogleIdClaims> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> GoogleOAuthAccount {
        GoogleOAuthAccount {
            account_id: "acct".to_string(),
            email: Some("user@example.com".to_string()),
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: None,
            scope: "https://www.googleapis.com/auth/calendar.events.readonly".to_string(),
        }
    }

    fn dt(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn external(event_id: &str) -> ExternalItemSource {
        ExternalItemSource {
            provider: CalendarProvider::Google,
            account_id: "acct".to_string(),
            calendar_id: "cal".to_string(),
            event_id: event_id.to_string(),
            instance_id: None,
            updated_at: None,
        }
    }

    fn imported_calendar() -> ImportedGoogleCalendar {
        ImportedGoogleCalendar {
            account_id: "acct".to_string(),
            account_email: Some("user@example.com".to_string()),
            calendar_id: "cal".to_string(),
            name: "Calendar".to_string(),
            color_index: 0,
            sync_token: None,
            full_sync: true,
            items: Vec::new(),
            deleted: Vec::new(),
        }
    }

    fn imported_google_scheme(name: &str) -> Scheme {
        let mut scheme = Scheme::new(name, 0);
        scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "acct".to_string(),
            account_email: Some("user@example.com".to_string()),
            calendar_id: "cal".to_string(),
            sync_token: None,
            read_only: true,
            last_synced_at: None,
        });
        scheme
    }

    #[test]
    fn google_event_to_item_preserves_calendar_identity_and_rrule() {
        let event = GoogleEvent {
            id: "event-1".to_string(),
            status: Some("confirmed".to_string()),
            summary: Some("Standup".to_string()),
            start: Some(GoogleEventDateTime {
                date: None,
                date_time: Some(dt("2026-05-18T13:00:00Z")),
            }),
            end: Some(GoogleEventDateTime {
                date: None,
                date_time: Some(dt("2026-05-18T13:30:00Z")),
            }),
            updated: Some(dt("2026-05-18T12:00:00Z")),
            recurrence: Some(vec!["RRULE:FREQ=WEEKLY;BYDAY=MO".to_string()]),
            recurring_event_id: None,
        };

        let item = google_event_to_item(&account(), "cal", &event).unwrap();

        assert_eq!(item.text, "Standup");
        assert_eq!(item.marker, ItemMarker::Checkbox);
        assert_eq!(item.start, Some(dt("2026-05-18T13:00:00Z")));
        assert_eq!(item.end, Some(dt("2026-05-18T13:30:00Z")));
        let external = item.external.unwrap();
        assert_eq!(external.provider, CalendarProvider::Google);
        assert_eq!(external.account_id, "acct");
        assert_eq!(external.calendar_id, "cal");
        assert_eq!(external.event_id, "event-1");
        assert_eq!(external.updated_at, Some(dt("2026-05-18T12:00:00Z")));
        let repeats = item.repeats.unwrap();
        assert_eq!(repeats.rrules, vec!["FREQ=WEEKLY;BYDAY=MO"]);
    }

    #[test]
    fn google_calendar_metadata_preserves_existing_local_color() {
        let mut scheme = Scheme::new("Local name", 4);
        let changed = apply_google_calendar_metadata(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Google name".to_string(),
                color_index: 1,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: Vec::new(),
                deleted: Vec::new(),
            },
            false,
        );

        assert!(changed);
        assert_eq!(scheme.name, "Local name");
        assert_eq!(scheme.color_index, 4);
        let SchemeSource::ImportedCalendar(source) = &scheme.source else {
            panic!("expected imported calendar source");
        };
        assert_eq!(source.provider, CalendarProvider::Google);
        assert_eq!(source.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(source.sync_token.as_deref(), Some("next"));
    }

    #[test]
    fn google_calendar_selected_existing_calendar_matches_locally_without_import_worker() {
        let account = account();
        let mut workspace = Workspace::new();
        let mut scheme = Scheme::new("Existing calendar", 2);
        let scheme_id = scheme.id;
        scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "user@example.com".to_string(),
            account_email: None,
            calendar_id: "cal".to_string(),
            sync_token: Some("token".to_string()),
            read_only: true,
            last_synced_at: None,
        });
        workspace.schemes.insert(scheme_id, scheme);

        assert_eq!(
            find_google_calendar_scheme_for_account(&workspace, &account, "cal"),
            Some(scheme_id)
        );
    }

    #[test]
    fn google_calendar_archived_calendar_does_not_count_as_added() {
        let account = account();
        let mut workspace = Workspace::new();
        let mut scheme = Scheme::new("Archived calendar", 2);
        let scheme_id = scheme.id;
        scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "user@example.com".to_string(),
            account_email: None,
            calendar_id: "cal".to_string(),
            sync_token: Some("token".to_string()),
            read_only: true,
            last_synced_at: None,
        });
        workspace.schemes.insert(scheme_id, scheme);
        workspace.mark_scheme_deleted(scheme_id);

        assert!(active_google_calendar_sources(&workspace).is_empty());
        assert_eq!(
            find_google_calendar_scheme_for_account(&workspace, &account, "cal"),
            None
        );
        assert_eq!(
            find_archived_google_calendar_scheme_for_account(&workspace, &account, "cal"),
            Some(scheme_id)
        );
    }

    #[test]
    fn active_google_calendar_scheme_ids_use_sidebar_order_and_skip_archived() {
        let mut workspace = Workspace::new();
        let root = workspace.root;
        let first = imported_google_scheme("First");
        let first_id = first.id;
        let second = imported_google_scheme("Second");
        let second_id = second.id;
        let archived = imported_google_scheme("Archived");
        let archived_id = archived.id;
        workspace.schemes.insert(first_id, first);
        workspace.schemes.insert(second_id, second);
        workspace.schemes.insert(archived_id, archived);
        workspace.folders.get_mut(&root).unwrap().children.extend([
            NodeRef::Scheme(second_id),
            NodeRef::Scheme(first_id),
            NodeRef::Scheme(archived_id),
        ]);
        workspace.mark_scheme_deleted(archived_id);

        assert_eq!(
            active_google_calendar_scheme_ids(&workspace, &imported_calendar()),
            vec![second_id, first_id]
        );
    }

    #[test]
    fn incremental_sync_updates_and_removes_matching_external_items() {
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Old");
        existing.external = Some(external("stay"));
        let existing_id = existing.id;
        let mut removed = Item::new("Remove");
        removed.external = Some(external("gone"));
        scheme.items = vec![existing, removed];

        let mut updated = Item::new("Updated");
        updated.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: vec![updated],
                deleted: vec![GoogleExternalEventKey {
                    event_id: "gone".to_string(),
                    instance_id: None,
                }],
            },
        );

        assert!(changed);
        assert_eq!(scheme.items.len(), 1);
        assert_eq!(scheme.items[0].id, existing_id);
        assert_eq!(scheme.items[0].text, "Updated");
        assert_eq!(scheme.items[0].external.as_ref().unwrap().event_id, "stay");
    }

    #[test]
    fn incremental_sync_without_item_changes_is_stable() {
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Same");
        existing.external = Some(external("stay"));
        let existing_id = existing.id;
        scheme.items = vec![existing];

        let mut imported = Item::new("Same");
        imported.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: vec![imported],
                deleted: Vec::new(),
            },
        );

        assert!(!changed);
        assert_eq!(scheme.items.len(), 1);
        assert_eq!(scheme.items[0].id, existing_id);
        assert_eq!(scheme.items[0].text, "Same");
    }

    #[test]
    fn incremental_sync_preserves_local_completion_state_for_same_event_time() {
        let start = dt("2026-05-18T13:00:00Z");
        let end = dt("2026-05-18T13:30:00Z");
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Same").with_start(start).with_end(end).done();
        existing.external = Some(external("stay"));
        scheme.items = vec![existing];

        let mut imported = Item::new("Same").with_start(start).with_end(end);
        imported.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: vec![imported],
                deleted: Vec::new(),
            },
        );

        assert!(!changed);
        assert!(scheme.items[0].single_state().is_done());
    }

    #[test]
    fn full_sync_preserves_local_completion_state_for_same_event_time() {
        let start = dt("2026-05-18T13:00:00Z");
        let end = dt("2026-05-18T13:30:00Z");
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Same").with_start(start).with_end(end).done();
        existing.external = Some(external("stay"));
        scheme.items = vec![existing];

        let mut imported = Item::new("Same").with_start(start).with_end(end);
        imported.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: true,
                items: vec![imported],
                deleted: Vec::new(),
            },
        );

        assert!(!changed);
        assert!(scheme.items[0].single_state().is_done());
    }

    #[test]
    fn sync_resets_completion_state_when_event_time_changes() {
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Same")
            .with_start(dt("2026-05-18T13:00:00Z"))
            .with_end(dt("2026-05-18T13:30:00Z"))
            .done();
        existing.external = Some(external("stay"));
        scheme.items = vec![existing];

        let mut imported = Item::new("Same")
            .with_start(dt("2026-05-19T13:00:00Z"))
            .with_end(dt("2026-05-19T13:30:00Z"));
        imported.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: vec![imported],
                deleted: Vec::new(),
            },
        );

        assert!(changed);
        assert!(!scheme.items[0].single_state().is_done());
    }
}
