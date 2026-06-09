use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use chrono::{DateTime, Utc};
use gpui::{Context, Window};
use knotq_model::{SyncAccountSettings, SyncAccountStatus};
use knotq_sync::AccountStatusResponse;
use serde::Deserialize;

use super::{
    KnotQApp, OnboardingPhase, SyncAccountAction, SyncAuthMode, SyncAuthStatus, SyncRunStatus,
};
use crate::app::google_oauth::{code_challenge, open_browser, random_token, wait_for_oauth_code};

const DEFAULT_SYNC_API_BASE: &str = "https://api.knotq.com";

/// KnotQ-hosted browser sign-in page. The app opens this with a loopback
/// `redirect_uri` + PKCE; the page signs the user in and hands back a one-time
/// authorization code redeemed via `POST /v1/auth/authorize/exchange`.
const SIGNIN_PAGE_URL: &str = "https://www.knotq.com/signin.html";
const ACCOUNT_PAGE_URL: &str = "https://www.knotq.com/account.html#signin";

#[derive(Deserialize)]
struct LoginResponse {
    #[serde(default)]
    session_id: Option<String>,
    user_id: String,
    workspace_id: String,
    email: String,
    #[serde(default = "default_supports_sync")]
    supports_sync: bool,
    bearer_token: String,
    expires_at: DateTime<Utc>,
    refresh_token: String,
    #[serde(default)]
    refresh_expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct CheckoutResponse {
    checkout_url: String,
}

/// Fresh credentials returned by `POST /v1/auth/refresh`.
pub(crate) struct RefreshedTokens {
    pub bearer_token: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_token: String,
    pub refresh_expires_at: Option<DateTime<Utc>>,
    pub workspace_id: String,
    pub supports_sync: bool,
}

/// Why a refresh attempt failed. `Unauthorized` means the refresh token is dead
/// (revoked/expired/replayed) and the user must sign in again; `Transient` is a
/// network/parse hiccup worth retrying on the next sync tick.
pub(crate) enum RefreshError {
    Unauthorized,
    Transient(anyhow::Error),
}

enum AccountStatusError {
    Unauthorized,
    Transient(anyhow::Error),
}

#[derive(Deserialize)]
struct LoginError {
    code: String,
}

impl KnotQApp {
    pub fn open_sync_sign_in(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.begin_browser_sign_in(SyncAuthMode::SignIn, false, cx);
    }

    pub fn open_sync_sign_in_for_onboarding(
        &mut self,
        mode: SyncAuthMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_browser_sign_in(mode, true, cx);
    }

    /// Start a browser-based sign-in: open the hosted sign-in page (with a loopback
    /// redirect + PKCE) and, on the redirect callback, exchange the one-time code
    /// for a session. The whole exchange runs on a background thread so the UI
    /// thread keeps painting while the browser is up.
    fn begin_browser_sign_in(
        &mut self,
        mode: SyncAuthMode,
        advance_onboarding: bool,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return;
        }
        self.sync_status_popover = None;
        self.close_repeat_popover();
        self.cancel_event_popup_without_commit(cx);
        self.search_open = false;

        let api_base = self
            .settings
            .sync_account
            .as_ref()
            .map(|account| account.api_base.clone())
            .unwrap_or_else(|| DEFAULT_SYNC_API_BASE.to_string());
        let api_base = match normalize_api_base(&api_base) {
            Ok(base) => base,
            Err(err) => {
                self.sync_auth_status = SyncAuthStatus::Error(err.to_string());
                cx.notify();
                return;
            }
        };

        self.sync_advance_onboarding_on_success = advance_onboarding;
        self.sync_auth_status = SyncAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result =
                        run_browser_sign_in(&api_base, mode).map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });
                Self::pump_sync_auth_worker(weak, cx, rx, |app, result, cx| {
                    app.finish_sync_sign_in(result, cx);
                })
                .await;
            },
        );
        self.sync_auth_task = Some(task);
        cx.notify();
    }

    /// Toggle the glanceable sync status popover anchored under the title-bar
    /// indicator. Clicking the indicator again (or anywhere outside) closes it.
    pub fn toggle_sync_status_popover(
        &mut self,
        anchor: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.sync_status_popover.is_some() {
            self.sync_status_popover = None;
        } else {
            self.sync_status_popover = Some(anchor);
        }
        cx.notify();
    }

    pub fn close_sync_status_popover(&mut self, cx: &mut Context<Self>) {
        if self.sync_status_popover.take().is_some() {
            cx.notify();
        }
    }

    /// Kick off a sync immediately (from the status popover) and close it.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        self.sync_status_popover = None;
        if self
            .settings
            .sync_account
            .as_ref()
            .is_some_and(|account| account.supports_sync)
        {
            self.service_bus.signal_sync();
        }
        cx.notify();
    }

    pub fn sign_out_sync_account(&mut self, cx: &mut Context<Self>) {
        if let Some(account) = self.settings.sync_account.take() {
            std::thread::spawn(move || {
                let _ = logout_sync_backend(&account);
            });
            // Don't leave a dead refresh token behind in the OS keychain.
            let _ = knotq_storage_json::secrets::delete_sync();
            self.save_app_settings();
        }
        self.sync_auth_status = SyncAuthStatus::Idle;
        self.sync_run_status = SyncRunStatus::Idle;
        self.sync_account_action = None;
        self.sync_status_popover = None;
        self.last_synced_at = None;
        cx.notify();
    }

    pub fn open_online_account_management(&mut self, cx: &mut Context<Self>) {
        match open_browser(ACCOUNT_PAGE_URL) {
            Ok(()) => {
                self.sync_auth_status = SyncAuthStatus::Idle;
            }
            Err(err) => {
                self.sync_auth_status =
                    SyncAuthStatus::Error(format!("Could not open account management: {err}"));
            }
        }
        cx.notify();
    }

    /// Arm the second-confirmation step for a destructive account action.
    pub fn prompt_sync_account_action(
        &mut self,
        action: SyncAccountAction,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return;
        }
        self.sync_account_action = Some(action);
        self.sync_auth_status = SyncAuthStatus::Idle;
        cx.notify();
    }

    /// Back out of a pending destructive action without performing it.
    pub fn dismiss_sync_account_action(&mut self, cx: &mut Context<Self>) {
        if self.sync_account_action.take().is_some() {
            self.sync_auth_status = SyncAuthStatus::Idle;
            cx.notify();
        }
    }

    /// Perform the pending destructive action after the user confirms it.
    pub fn confirm_sync_account_action(&mut self, cx: &mut Context<Self>) {
        match self.sync_account_action {
            Some(SyncAccountAction::CancelSubscription) => self.cancel_sync_subscription(cx),
            None => {}
        }
    }

    fn cancel_sync_subscription(&mut self, cx: &mut Context<Self>) {
        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return;
        }
        let Some(account) = self.settings.sync_account.clone() else {
            return;
        };
        self.sync_auth_status = SyncAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result =
                        cancel_subscription_backend(&account).map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });
                Self::pump_sync_auth_worker(weak, cx, rx, |app, result, cx| {
                    app.finish_cancel_subscription(result, cx);
                })
                .await;
            },
        );
        self.sync_auth_task = Some(task);
        cx.notify();
    }

    fn finish_cancel_subscription(
        &mut self,
        result: Result<SyncAccountSettings, String>,
        cx: &mut Context<Self>,
    ) {
        self.sync_auth_task = None;
        match result {
            Ok(account) => {
                // Provider-backed cancellation may keep entitlement active until
                // the paid period ends. Install the re-credentialed account either
                // way so local state matches the backend's current decision.
                let supports_sync = account.supports_sync;
                self.settings.sync_account = Some(account);
                self.sync_account_action = None;
                self.sync_auth_status = SyncAuthStatus::Idle;
                if !supports_sync {
                    self.sync_run_status = SyncRunStatus::Idle;
                }
                self.save_app_settings();
            }
            Err(message) => {
                self.sync_auth_status = SyncAuthStatus::Error(message);
            }
        }
        cx.notify();
    }

    /// Ask the API to create a provider checkout tied to this account, then open the
    /// returned hosted checkout URL in the browser.
    pub fn open_subscription_checkout(&mut self, cx: &mut Context<Self>) {
        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return;
        }
        let Some(account) = self.settings.sync_account.clone() else {
            return;
        };
        self.sync_auth_status = SyncAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = create_subscription_checkout_backend(&account)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });
                Self::pump_sync_auth_worker(weak, cx, rx, move |app, result, cx| {
                    app.finish_open_subscription_checkout(result, cx);
                })
                .await;
            },
        );
        self.sync_auth_task = Some(task);
        cx.notify();
    }

    fn finish_open_subscription_checkout(
        &mut self,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        self.sync_auth_task = None;
        match result {
            Ok(url) => match open_browser(&url) {
                Ok(()) => {
                    self.sync_auth_status = SyncAuthStatus::Idle;
                    // The purchase finishes in the browser with no callback into
                    // the app, so quietly poll entitlement until the webhook lands
                    // and sync turns on by itself — no manual re-check needed.
                    self.start_subscription_status_poll(cx);
                }
                Err(err) => {
                    self.sync_auth_status =
                        SyncAuthStatus::Error(format!("Could not open the checkout page: {err}"));
                }
            },
            Err(message) => {
                self.sync_auth_status = SyncAuthStatus::Error(message);
            }
        }
        cx.notify();
    }

    /// Refresh the general account/subscription status shown in Settings.
    pub fn refresh_account_status(&mut self, cx: &mut Context<Self>) {
        self.refresh_account_status_with_options(false, cx);
    }

    /// After the checkout opens in the browser, quietly re-check entitlement on a
    /// timer until the purchase webhook lands and sync turns on — so the user
    /// doesn't have to return and press anything. Runs in the background without
    /// touching `sync_auth_status`, so the rest of the account UI stays usable.
    fn start_subscription_status_poll(&mut self, cx: &mut Context<Self>) {
        let already_syncing = self
            .settings
            .sync_account
            .as_ref()
            .is_some_and(|account| account.supports_sync);
        if self.settings.sync_account.is_none() || already_syncing {
            return;
        }

        // Re-check every few seconds for several minutes. The status endpoint
        // reconciles with the billing provider on each call, so entitlement flips
        // within one tick of the payment completing; the long window just covers a
        // user who lingers on the checkout page before paying.
        const POLL_INTERVAL: StdDuration = StdDuration::from_secs(5);
        const MAX_ATTEMPTS: usize = 60;

        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                for _ in 0..MAX_ATTEMPTS {
                    cx.background_executor().timer(POLL_INTERVAL).await;

                    // Snapshot the freshest account each tick (a prior tick or a
                    // manual refresh may have rotated its tokens). Stop once the
                    // account is gone or sync has turned on.
                    let account = match weak.update(cx, |app, _| {
                        app.settings
                            .sync_account
                            .clone()
                            .filter(|account| !account.supports_sync)
                    }) {
                        Ok(Some(account)) => account,
                        Ok(None) => break,
                        Err(_) => return, // app entity dropped
                    };

                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(refresh_account_status_backend(account));
                    });

                    // Await the worker without blocking the UI executor.
                    let result = loop {
                        match rx.try_recv() {
                            Ok(result) => break Some(result),
                            Err(mpsc::TryRecvError::Empty) => {
                                cx.background_executor()
                                    .timer(StdDuration::from_millis(100))
                                    .await;
                            }
                            Err(mpsc::TryRecvError::Disconnected) => break None,
                        }
                    };

                    // A transient error (offline, 5xx) just means retry next tick.
                    let Some(Ok(updated)) = result else {
                        continue;
                    };

                    let enabled = updated.supports_sync;
                    let applied = weak.update(cx, |app, cx| {
                        app.settings.sync_account = Some(updated);
                        app.save_app_settings();
                        if enabled {
                            app.service_bus.signal_sync();
                        }
                        cx.notify();
                    });
                    if applied.is_err() || enabled {
                        break;
                    }
                }

                let _ = weak.update(cx, |app, _| {
                    app.sync_subscription_poll_task = None;
                });
            },
        );
        self.sync_subscription_poll_task = Some(task);
    }

    fn refresh_account_status_with_options(&mut self, require_sync: bool, cx: &mut Context<Self>) {
        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return;
        }
        let Some(account) = self.settings.sync_account.clone() else {
            return;
        };
        self.sync_auth_status = SyncAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result =
                        refresh_account_status_backend(account).map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });
                Self::pump_sync_auth_worker(weak, cx, rx, move |app, result, cx| {
                    app.finish_refresh_account_status(result, require_sync, cx);
                })
                .await;
            },
        );
        self.sync_auth_task = Some(task);
        cx.notify();
    }

    fn finish_refresh_account_status(
        &mut self,
        result: Result<SyncAccountSettings, String>,
        require_sync: bool,
        cx: &mut Context<Self>,
    ) {
        self.sync_auth_task = None;
        match result {
            Ok(account) => {
                let supports_sync = account.supports_sync;
                self.settings.sync_account = Some(account);
                self.save_app_settings();
                if !require_sync || supports_sync {
                    self.sync_auth_status = SyncAuthStatus::Idle;
                    if supports_sync {
                        self.service_bus.signal_sync();
                    }
                } else {
                    self.sync_auth_status = SyncAuthStatus::Error(
                        "No active subscription found yet. If you just paid, wait a moment and try again."
                            .to_string(),
                    );
                }
            }
            Err(message) => {
                self.sync_auth_status = SyncAuthStatus::Error(message);
            }
        }
        cx.notify();
    }

    /// Poll a background auth worker's channel from the UI executor and hand the
    /// result to `finish` on the app entity. Shared by the browser sign-in and the
    /// account-management actions.
    async fn pump_sync_auth_worker<T: Send + 'static>(
        weak: gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        rx: mpsc::Receiver<Result<T, String>>,
        finish: impl Fn(&mut Self, Result<T, String>, &mut Context<Self>) + Copy + 'static,
    ) {
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    let _ = weak.update(cx, |app, cx| finish(app, result, cx));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    cx.background_executor()
                        .timer(StdDuration::from_millis(100))
                        .await;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = weak.update(cx, |app, cx| {
                        finish(app, Err("Sync sign-in worker stopped".to_string()), cx);
                    });
                    break;
                }
            }
        }
    }

    fn finish_sync_sign_in(
        &mut self,
        result: Result<SyncAccountSettings, String>,
        cx: &mut Context<Self>,
    ) {
        self.sync_auth_task = None;
        match result {
            Ok(account) => {
                let advance_onboarding = self.sync_advance_onboarding_on_success;
                self.sync_advance_onboarding_on_success = false;
                self.settings.sync_account = Some(account);
                self.sync_auth_status = SyncAuthStatus::Idle;
                if advance_onboarding && self.show_onboarding {
                    self.onboarding_phase = OnboardingPhase::Guide;
                    self.onboarding_page = 0;
                }
                self.save_app_settings();
                self.service_bus.signal_sync();
            }
            Err(message) => {
                self.sync_auth_status = SyncAuthStatus::Error(message);
            }
        }
        cx.notify();
    }
}

/// Run the full browser sign-in on a background thread: open the hosted sign-in
/// page against a loopback redirect with PKCE, wait for the redirect callback, and
/// exchange the one-time authorization code for a session. Blocking (`ureq` + a
/// loopback `TcpListener`), so callers run it off the UI thread.
fn run_browser_sign_in(api_base: &str, mode: SyncAuthMode) -> Result<SyncAccountSettings> {
    let base = normalize_api_base(api_base)?;
    let listener = TcpListener::bind("127.0.0.1:0").context("bind sign-in loopback listener")?;
    listener
        .set_nonblocking(true)
        .context("make sign-in loopback listener nonblocking")?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let state = random_token(32);
    // PKCE: the verifier never leaves the app; only its challenge rides the URL, so
    // an intercepted authorization code is useless without this process.
    let code_verifier = random_token(64);
    let challenge = code_challenge(&code_verifier);
    let url = sign_in_authorize_url(&redirect_uri, &state, mode, &base, &challenge);

    open_browser(&url)?;
    let code = wait_for_oauth_code(
        &listener,
        &state,
        StdDuration::from_secs(300),
        "You're signed in to KnotQ. You can close this tab and return to the app.",
        "Sign-in did not complete. You can close this tab and return to KnotQ.",
    )?;
    exchange_authorize_code(&base, &code, &code_verifier)
}

/// Build the hosted sign-in URL. `api` lets the page (and the later exchange) talk
/// to the same backend the app is configured for (prod, local dev, or self-host).
fn sign_in_authorize_url(
    redirect_uri: &str,
    state: &str,
    mode: SyncAuthMode,
    api_base: &str,
    code_challenge: &str,
) -> String {
    let mode_param = match mode {
        SyncAuthMode::SignIn => "signin",
        SyncAuthMode::CreateAccount => "create",
    };
    let params = [
        ("redirect_uri", redirect_uri),
        ("state", state),
        ("mode", mode_param),
        ("api", api_base),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256"),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{SIGNIN_PAGE_URL}?{query}")
}

/// Redeem the one-time authorization code (with the PKCE verifier) for a session.
fn exchange_authorize_code(
    api_base: &str,
    code: &str,
    code_verifier: &str,
) -> Result<SyncAccountSettings> {
    let base = normalize_api_base(api_base)?;
    let url = format!("{base}/v1/auth/authorize/exchange");
    let response = match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .send_json(serde_json::json!({ "code": code, "code_verifier": code_verifier }))
    {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => {
            let code = response
                .into_json::<LoginError>()
                .map(|error| error.code)
                .unwrap_or_else(|_| "unauthorized".to_string());
            return Err(anyhow!(authorize_error_message(&code)));
        }
        Err(error) => return Err(anyhow!("Could not reach the sync API: {error}")),
    };
    let session: LoginResponse = response
        .into_json()
        .context("parse sync sign-in response")?;
    sync_account_settings_from_session(base, session)
}

fn sync_account_settings_from_session(
    api_base: String,
    session: LoginResponse,
) -> Result<SyncAccountSettings> {
    if session.refresh_token.is_empty() {
        return Err(anyhow!("sync response missing refresh token"));
    }
    Ok(SyncAccountSettings {
        api_base,
        user_id: session.user_id,
        session_id: session.session_id,
        workspace_id: Some(session.workspace_id),
        email: session.email,
        supports_sync: session.supports_sync,
        bearer_token: session.bearer_token,
        expires_at: session.expires_at,
        refresh_token: Some(session.refresh_token),
        refresh_expires_at: session.refresh_expires_at,
        account_status: Some(SyncAccountStatus::from_supports_sync(session.supports_sync)),
    })
}

/// Exchange a refresh token for a fresh access token (and a rotated refresh
/// token). Runs on a background thread (blocking `ureq`).
pub(crate) fn refresh_sync_backend(
    api_base: &str,
    refresh_token: &str,
) -> Result<RefreshedTokens, RefreshError> {
    let base = normalize_api_base(api_base).map_err(RefreshError::Transient)?;
    let url = format!("{base}/v1/auth/refresh");
    let response = match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .send_json(serde_json::json!({ "refresh_token": refresh_token }))
    {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => return Err(RefreshError::Unauthorized),
        Err(error) => {
            return Err(RefreshError::Transient(anyhow!(
                "could not refresh sync session: {error}"
            )))
        }
    };
    let session: LoginResponse = response
        .into_json()
        .context("parse sync refresh response")
        .map_err(RefreshError::Transient)?;
    if session.refresh_token.is_empty() {
        return Err(RefreshError::Transient(anyhow!(
            "sync refresh response missing refresh token"
        )));
    }
    Ok(RefreshedTokens {
        bearer_token: session.bearer_token,
        expires_at: session.expires_at,
        refresh_token: session.refresh_token,
        refresh_expires_at: session.refresh_expires_at,
        workspace_id: session.workspace_id,
        supports_sync: session.supports_sync,
    })
}

fn logout_sync_backend(account: &SyncAccountSettings) -> Result<()> {
    let url = format!("{}/v1/auth/logout", normalize_api_base(&account.api_base)?);
    match ureq::post(&url)
        .timeout(StdDuration::from_secs(5))
        .set("authorization", &format!("Bearer {}", account.bearer_token))
        .send_json(serde_json::json!({}))
    {
        Ok(_) | Err(ureq::Error::Status(401, _)) => Ok(()),
        Err(error) => Err(anyhow!("Could not revoke sync session: {error}")),
    }
}

/// Turn off the sync entitlement for the account (keeps the account + data). The
/// backend rotates the session, so this returns fresh credentials to install.
fn cancel_subscription_backend(account: &SyncAccountSettings) -> Result<SyncAccountSettings> {
    let base = normalize_api_base(&account.api_base)?;
    let url = format!("{base}/v1/auth/subscription/cancel");
    let response = match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .set("authorization", &format!("Bearer {}", account.bearer_token))
        .send_json(serde_json::json!({}))
    {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => {
            let code = response
                .into_json::<LoginError>()
                .map(|error| error.code)
                .unwrap_or_else(|_| "cancel_failed".to_string());
            return Err(anyhow!(account_action_error_message(&code)));
        }
        Err(error) => return Err(anyhow!("Could not reach the sync API: {error}")),
    };
    let session: LoginResponse = response
        .into_json()
        .context("parse sync subscription cancel response")?;
    sync_account_settings_from_session(base, session)
}

/// Create a provider-hosted checkout URL for the signed-in account.
fn create_subscription_checkout_backend(account: &SyncAccountSettings) -> Result<String> {
    let base = normalize_api_base(&account.api_base)?;
    let url = format!("{base}/v1/billing/lemonsqueezy/checkout");
    let response = match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .set("authorization", &format!("Bearer {}", account.bearer_token))
        .send_json(serde_json::json!({
            "user_id": account.user_id,
            "email": account.email,
        })) {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => {
            let code = response
                .into_json::<LoginError>()
                .map(|error| error.code)
                .unwrap_or_else(|_| "checkout_failed".to_string());
            return Err(anyhow!(account_action_error_message(&code)));
        }
        Err(error) => return Err(anyhow!("Could not reach the sync API: {error}")),
    };
    let checkout: CheckoutResponse = response
        .into_json()
        .context("parse sync subscription checkout response")?;
    if checkout.checkout_url.trim().is_empty() {
        return Err(anyhow!("The sync API did not return a checkout URL."));
    }
    Ok(checkout.checkout_url)
}

fn refresh_account_status_backend(mut account: SyncAccountSettings) -> Result<SyncAccountSettings> {
    match fetch_account_status_backend(&account) {
        Ok(status) => {
            apply_account_status_response(&mut account, status);
            return Ok(account);
        }
        Err(AccountStatusError::Unauthorized) => {}
        Err(AccountStatusError::Transient(error)) => return Err(error),
    }

    let Some(refresh_token) = account.refresh_token.clone() else {
        return Err(anyhow!("Your sync session expired. Please sign in again."));
    };
    let tokens = match refresh_sync_backend(&account.api_base, &refresh_token) {
        Ok(tokens) => tokens,
        Err(RefreshError::Unauthorized) => {
            return Err(anyhow!("Your sync session expired. Please sign in again."));
        }
        Err(RefreshError::Transient(error)) => return Err(error),
    };
    account.bearer_token = tokens.bearer_token;
    account.expires_at = tokens.expires_at;
    account.refresh_token = Some(tokens.refresh_token);
    account.refresh_expires_at = tokens.refresh_expires_at;
    account.workspace_id = Some(tokens.workspace_id);
    account.supports_sync = tokens.supports_sync;
    account.account_status = Some(SyncAccountStatus::from_supports_sync(tokens.supports_sync));

    let status = fetch_account_status_backend(&account).map_err(|err| match err {
        AccountStatusError::Unauthorized => {
            anyhow!("Your sync session expired. Please sign in again.")
        }
        AccountStatusError::Transient(error) => error,
    })?;
    apply_account_status_response(&mut account, status);
    Ok(account)
}

fn fetch_account_status_backend(
    account: &SyncAccountSettings,
) -> Result<AccountStatusResponse, AccountStatusError> {
    let base = normalize_api_base(&account.api_base).map_err(AccountStatusError::Transient)?;
    let url = format!("{base}/v1/auth/account/status");
    let response = match ureq::get(&url)
        .timeout(StdDuration::from_secs(10))
        .set("authorization", &format!("Bearer {}", account.bearer_token))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => return Err(AccountStatusError::Unauthorized),
        Err(ureq::Error::Status(404, _)) => {
            return Err(AccountStatusError::Transient(anyhow!(
                "The sync API does not support account status yet."
            )));
        }
        Err(ureq::Error::Status(_, response)) => {
            let code = response
                .into_json::<LoginError>()
                .map(|error| error.code)
                .unwrap_or_else(|_| "status_failed".to_string());
            return Err(AccountStatusError::Transient(anyhow!(
                account_status_error_message(&code)
            )));
        }
        Err(error) => {
            return Err(AccountStatusError::Transient(anyhow!(
                "Could not reach the sync API: {error}"
            )));
        }
    };
    response
        .into_json()
        .context("parse sync account status response")
        .map_err(AccountStatusError::Transient)
}

fn apply_account_status_response(account: &mut SyncAccountSettings, status: AccountStatusResponse) {
    account.user_id = status.user_id.to_string();
    account.workspace_id = Some(status.workspace_id.to_string());
    account.email = status.email.clone();
    account.supports_sync = status.supports_sync;
    account.account_status = Some(sync_account_status_from_response(status));
}

fn sync_account_status_from_response(status: AccountStatusResponse) -> SyncAccountStatus {
    SyncAccountStatus {
        level: status.level,
        subscribed: status.subscribed,
        supports_sync: status.supports_sync,
        subscription_status: status.subscription_status.or_else(|| {
            Some(
                if status.subscribed {
                    "active"
                } else {
                    "inactive"
                }
                .to_string(),
            )
        }),
        subscription_provider: status.subscription_provider,
        current_period_end: status.current_period_end,
        checked_at: Some(status.checked_at.unwrap_or_else(Utc::now)),
    }
}

fn account_status_error_message(code: &str) -> &'static str {
    match code {
        "unauthorized" => "Your sync session expired. Sign in again, then retry.",
        "billing_api_not_configured" => "Subscription cancellation is not configured yet.",
        "cancel_in_app_store" => {
            "Manage this App Store subscription from your Apple account subscriptions."
        }
        _ => "The request to the sync API failed.",
    }
}

fn account_action_error_message(code: &str) -> &'static str {
    match code {
        "unauthorized" => "Your sync session expired. Sign in again, then retry.",
        "delete_confirmation_mismatch" => "Could not confirm the account. Please try again.",
        "billing_api_not_configured" | "billing_checkout_not_configured" => {
            "Subscription checkout is not configured yet."
        }
        "billing_provider_error" => "Subscription checkout is temporarily unavailable.",
        "email_mismatch" | "forbidden" => "This checkout does not match the signed-in account.",
        "cancel_in_app_store" => {
            "Manage this App Store subscription from your Apple account subscriptions."
        }
        _ => "The request to the sync API failed.",
    }
}

/// Percent-encode a query value, escaping everything outside the RFC 3986
/// unreserved set.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn default_supports_sync() -> bool {
    true
}

fn normalize_api_base(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!("Enter a sync API URL."));
    }
    if let Some(after_scheme) = trimmed.strip_prefix("http://") {
        // Bearer tokens must never travel in cleartext, so plain http is only
        // permitted to a loopback host (local dev / self-hosted Worker on-box).
        if !is_loopback_http_authority(after_scheme) {
            return Err(anyhow!(
                "Sync API must use https:// (plain http is only allowed for localhost)."
            ));
        }
    } else if trimmed.strip_prefix("https://").is_none() {
        return Err(anyhow!("Sync API must start with http:// or https://."));
    }
    Ok(trimmed.to_string())
}

/// Whether the authority following `http://` names a loopback host. Accepts
/// `127.0.0.1`, `localhost`, and the IPv6 literal `[::1]`, with or without a port
/// or trailing path.
fn is_loopback_http_authority(after_scheme: &str) -> bool {
    let authority = after_scheme.split('/').next().unwrap_or("");
    // Defensive: drop any userinfo ("user:pass@host").
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 literal: the host is inside the brackets.
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// The authorization code is minted by the hosted page and redeemed here; the only
/// failures the app surfaces are a stale/replayed code, so keep the guidance simple.
fn authorize_error_message(code: &str) -> &'static str {
    match code {
        "invalid_authorization_code" | "authorization_code_expired" | "invalid_code_challenge" => {
            "Sign-in could not be completed. Please try signing in again."
        }
        _ => "Sign in failed.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_api_base_accepts_https() {
        assert_eq!(
            normalize_api_base("https://sync.example.com").unwrap(),
            "https://sync.example.com"
        );
        // Trailing slashes are trimmed.
        assert_eq!(
            normalize_api_base("https://sync.example.com/").unwrap(),
            "https://sync.example.com"
        );
    }

    #[test]
    fn normalize_api_base_allows_http_only_for_loopback() {
        assert_eq!(
            normalize_api_base("http://127.0.0.1:8787").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert_eq!(
            normalize_api_base("http://localhost:8787").unwrap(),
            "http://localhost:8787"
        );
        assert_eq!(
            normalize_api_base("http://[::1]:8787").unwrap(),
            "http://[::1]:8787"
        );
    }

    #[test]
    fn normalize_api_base_rejects_plain_http_to_remote_host() {
        assert!(normalize_api_base("http://example.com").is_err());
        assert!(normalize_api_base("http://sync.example.com:8787").is_err());
        // A loopback-looking name embedded elsewhere must not slip through.
        assert!(normalize_api_base("http://localhost.evil.com").is_err());
    }

    #[test]
    fn normalize_api_base_rejects_other_schemes_and_empty() {
        assert!(normalize_api_base("ftp://example.com").is_err());
        assert!(normalize_api_base("example.com").is_err());
        assert!(normalize_api_base("   ").is_err());
    }
}
