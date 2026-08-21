use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration as StdDuration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use base64::Engine as _;
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use gpui::Context;
use knotq_model::{
    CalendarDateTime, CalendarProvider, ExternalItemSource, FolderId, GoogleOAuthAccount,
    ImportedCalendarSource, Item, ItemMarker, NodeRef, Scheme, SchemeId, SchemeSource, Workspace,
};
use knotq_storage_json::data_dir;
use rand::distributions::{Alphanumeric, DistString};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::workspace_ops::{
    emails_match, google_account_can_sync, google_account_matches_calendar_source,
    google_calendar_source_target_label,
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
/// Calendar reads the import cannot work without, each paired with the broader
/// scopes Google treats as covering it — an account connected back when KnotQ
/// asked for `calendar.readonly` keeps working without another consent round.
const GOOGLE_REQUIRED_CALENDAR_SCOPES: &[(&str, &[&str])] = &[
    (
        "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
        &[
            "https://www.googleapis.com/auth/calendar.calendarlist",
            "https://www.googleapis.com/auth/calendar.readonly",
            "https://www.googleapis.com/auth/calendar",
        ],
    ),
    (
        "https://www.googleapis.com/auth/calendar.events.readonly",
        &[
            "https://www.googleapis.com/auth/calendar.events",
            "https://www.googleapis.com/auth/calendar.readonly",
            "https://www.googleapis.com/auth/calendar",
        ],
    ),
];
/// How long to wait for the browser to finish sending its redirect request once
/// the connection has been accepted.
const CALLBACK_READ_TIMEOUT: StdDuration = StdDuration::from_secs(10);
const GOOGLE_OAUTH_LOG_FILE: &str = "knotq-google.log";
const IMPORTED_GOOGLE_CALENDAR_SCHEME_NAME: &str = "Google Calendar";
const GOOGLE_CALENDAR_BACKGROUND_SYNC_INTERVAL_SECS: u64 = 10 * 60;
const GOOGLE_OAUTH_CLIENT_ID_ENV: &str = "KNOTQ_GOOGLE_OAUTH_CLIENT_ID";
const GOOGLE_OAUTH_CLIENT_SECRET_ENV: &str = "KNOTQ_GOOGLE_OAUTH_CLIENT_SECRET";
const COMPILED_GOOGLE_OAUTH_CLIENT_ID: Option<&str> = option_env!("KNOTQ_GOOGLE_OAUTH_CLIENT_ID");
const COMPILED_GOOGLE_OAUTH_CLIENT_SECRET: Option<&str> =
    option_env!("KNOTQ_GOOGLE_OAUTH_CLIENT_SECRET");

#[derive(Clone)]
pub(crate) struct GoogleOAuthConfig {
    client_id: String,
    client_secret: String,
}

#[derive(Clone)]
pub(crate) struct ExistingGoogleCalendarSource {
    account_id: String,
    account_email: Option<String>,
    calendar_id: String,
    sync_token: Option<String>,
}

pub(crate) struct GoogleCalendarImportResult {
    accounts: Vec<GoogleOAuthAccount>,
    calendars: Vec<ImportedGoogleCalendar>,
    failures: Vec<String>,
}

pub(crate) struct GoogleCalendarPickerLoadResult {
    accounts: Vec<GoogleOAuthAccount>,
    picker_accounts: Vec<GoogleCalendarPickerAccount>,
}

pub(crate) struct ImportedGoogleCalendar {
    account_id: String,
    account_email: Option<String>,
    calendar_id: String,
    name: String,
    color_index: u8,
    sync_token: Option<String>,
    full_sync: bool,
    items: Vec<Item>,
    deleted: Vec<GoogleExternalEventKey>,
    recurrence_exdates: Vec<GoogleRecurrenceExdate>,
}

#[derive(Clone, Copy)]
pub(crate) enum GoogleCalendarImportMode {
    MissingOnly,
    ExistingOnly,
}

pub(crate) struct GoogleCalendarApplyResult {
    content_changed: bool,
}

fn google_oauth_log(message: impl AsRef<str>) {
    knotq_storage_json::append_diagnostic_line(
        &data_dir(),
        GOOGLE_OAUTH_LOG_FILE,
        &format!(
            "[{}] {}",
            Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
            message.as_ref()
        ),
    );
}

/// The "cancelled" sentinel message and the specific `access_denied` instance of
/// the provider-error message. Produced from the same localized templates used
/// at the `bail!` sites so the substring match below stays correct under any
/// active locale.
fn google_oauth_error_cancelled() -> &'static str {
    knotq_l10n::t("google.oauth.error.cancelled")
}

fn google_oauth_error_timeout() -> &'static str {
    knotq_l10n::t("google.oauth.error.timeout")
}

fn google_oauth_error_access_denied() -> String {
    knotq_l10n::t_with("google.oauth.error.provider_error", &[("error", "access_denied")])
}

/// Whether a failed browser flow was the user's own doing — they cancelled from
/// inside KnotQ, or declined on Google's screen. Those need no error modal; a
/// timeout deliberately does *not* count, because the usual reason the callback
/// never arrives is that Google showed the user an error page we cannot see.
fn is_google_oauth_user_cancelled(err: &str) -> bool {
    err.contains(google_oauth_error_cancelled())
        || err.contains(google_oauth_error_access_denied().as_str())
}

/// The calendar scopes KnotQ asked for that a grant does not actually cover.
///
/// Google's consent screen lists each calendar permission as its own checkbox,
/// and a user can finish the flow having ticked none of them: the exchange then
/// succeeds with nothing but `openid`/`email`, and every Calendar call afterwards
/// fails with `ACCESS_TOKEN_SCOPE_INSUFFICIENT`. Comparing the granted scopes
/// against what we need is what turns that into a message while the user is
/// still in front of the browser.
pub(crate) fn missing_google_calendar_scopes(granted: &str) -> Vec<&'static str> {
    let granted = granted.split_whitespace().collect::<HashSet<_>>();
    GOOGLE_REQUIRED_CALENDAR_SCOPES
        .iter()
        .filter(|(scope, broader)| {
            !granted.contains(scope) && !broader.iter().any(|scope| granted.contains(scope))
        })
        .map(|(scope, _)| *scope)
        .collect()
}

fn google_account_label(account: &GoogleOAuthAccount) -> &str {
    account
        .email
        .as_deref()
        .filter(|email| !email.trim().is_empty())
        .unwrap_or(&account.account_id)
}

fn google_import_mode_label(mode: GoogleCalendarImportMode) -> &'static str {
    match mode {
        GoogleCalendarImportMode::MissingOnly => "missing_only",
        GoogleCalendarImportMode::ExistingOnly => "existing_only",
    }
}

fn is_terminal_google_refresh_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}");
    message.contains("invalid_grant")
}

pub(crate) struct GoogleCalendarBackgroundSnapshot {
    config: GoogleOAuthConfig,
    accounts: Vec<GoogleOAuthAccount>,
    sources: Vec<ExistingGoogleCalendarSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GoogleExternalEventKey {
    event_id: String,
    instance_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GoogleRecurrenceExdate {
    event_id: String,
    original_start: CalendarDateTime,
}

#[derive(Deserialize)]
pub(crate) struct GoogleTokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    scope: Option<String>,
    id_token: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct GoogleIdClaims {
    sub: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleCalendarListResponse {
    next_page_token: Option<String>,
    items: Vec<GoogleCalendarListEntry>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleCalendarListEntry {
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
pub(crate) struct GoogleEventsResponse {
    next_page_token: Option<String>,
    next_sync_token: Option<String>,
    items: Vec<GoogleEvent>,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleEvent {
    id: String,
    status: Option<String>,
    summary: Option<String>,
    start: Option<GoogleEventDateTime>,
    end: Option<GoogleEventDateTime>,
    updated: Option<DateTime<Utc>>,
    recurrence: Option<Vec<String>>,
    recurring_event_id: Option<String>,
    original_start_time: Option<GoogleEventDateTime>,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleEventDateTime {
    date: Option<NaiveDate>,
    date_time: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub(crate) struct GoogleApiError {
    status: Option<u16>,
    message: String,
}

impl std::fmt::Display for GoogleApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GoogleApiError {}

fn google_oauth_config_from_build() -> Result<GoogleOAuthConfig> {
    let client_id = google_oauth_client_id_from_compiled(COMPILED_GOOGLE_OAUTH_CLIENT_ID)
        .with_context(|| format!("{GOOGLE_OAUTH_CLIENT_ID_ENV} must be set at compile time"))?;
    let client_secret =
        google_oauth_client_secret_from_compiled(COMPILED_GOOGLE_OAUTH_CLIENT_SECRET)
            .with_context(|| {
                format!("{GOOGLE_OAUTH_CLIENT_SECRET_ENV} must be set at compile time")
            })?;
    Ok(GoogleOAuthConfig {
        client_id,
        client_secret,
    })
}

fn google_oauth_config_for_existing_accounts(
    _accounts: &[GoogleOAuthAccount],
) -> Result<GoogleOAuthConfig> {
    google_oauth_config_from_build()
}

fn google_oauth_client_id_from_compiled(compiled: Option<&str>) -> Option<String> {
    compiled
        .map(str::trim)
        .filter(|client_id| !client_id.is_empty())
        .map(ToOwned::to_owned)
}

fn google_oauth_client_secret_from_compiled(compiled: Option<&str>) -> Option<String> {
    compiled
        .map(str::trim)
        .filter(|client_secret| !client_secret.is_empty())
        .map(ToOwned::to_owned)
}
mod app_methods;
mod calendar;
mod network;
#[cfg(test)]
mod tests;

pub(crate) use calendar::*;
pub(crate) use network::*;
