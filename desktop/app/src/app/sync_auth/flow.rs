use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use gpui::{Context, Window};
use knotq_model::{SyncAccountSettings, SyncAccountStatus};

use super::tokens::logout_sync_backend;
use super::{
    default_sync_api_base, normalize_api_base, percent_encode, sync_web_base, LoginError,
    LoginResponse,
};
use crate::app::google_oauth::{code_challenge, open_browser, random_token, wait_for_oauth_code};
use crate::app::{KnotQApp, SyncAuthMode, SyncAuthStatus, SyncRunStatus};

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
    /// Start (or restart) the browser sign-in's cancel token. Cancels any flow
    /// already in progress so re-clicking "Sign in" during the polling window
    /// aborts the stale loopback wait and relaunches the browser.
    fn begin_sync_sign_in_flow(&mut self) -> Arc<AtomicBool> {
        self.cancel_sync_sign_in_flow();
        let cancel_token = Arc::new(AtomicBool::new(false));
        self.sync_auth_cancel_token = Some(cancel_token.clone());
        cancel_token
    }

    fn cancel_sync_sign_in_flow(&mut self) {
        if let Some(cancel_token) = self.sync_auth_cancel_token.take() {
            cancel_token.store(true, Ordering::SeqCst);
            self.sync_auth_task = None;
        }
    }

    /// Clears the task/token for a finished worker, but only if it owns the
    /// current flow — a result from a cancelled-and-relaunched flow is stale and
    /// must not clobber the live one's state. Returns whether the caller should
    /// apply the result.
    fn finish_sync_sign_in_flow(&mut self, cancel_token: &Arc<AtomicBool>) -> bool {
        match self.sync_auth_cancel_token.as_ref() {
            Some(current) if Arc::ptr_eq(current, cancel_token) => {
                self.sync_auth_cancel_token = None;
                self.sync_auth_task = None;
                true
            }
            _ => false,
        }
    }

    fn begin_browser_sign_in(
        &mut self,
        mode: SyncAuthMode,
        advance_onboarding: bool,
        cx: &mut Context<Self>,
    ) {
        self.sync_status_popover = None;
        self.close_repeat_popover();
        self.cancel_event_popup_without_commit(cx);
        self.search_open = false;

        let api_base = self
            .settings
            .sync_account
            .as_ref()
            .map(|account| account.api_base.clone())
            .unwrap_or_else(default_sync_api_base);
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
        let cancel_token = self.begin_sync_sign_in_flow();
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                let worker_token = cancel_token.clone();
                std::thread::spawn(move || {
                    let result = run_browser_sign_in(&api_base, mode, &worker_token)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });
                Self::pump_sync_auth_worker(weak, cx, rx, move |app, result, cx| {
                    app.finish_sync_sign_in(result, &cancel_token, cx);
                })
                .await;
            },
        );
        self.sync_auth_task = Some(task);
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
        self.sync_status_quiet_task = None;
        self.last_synced_at = None;
        cx.notify();
    }

    /// Poll a background auth worker's channel from the UI executor and hand the
    /// result to `finish` on the app entity. Shared by the browser sign-in and the
    /// account-management actions.
    pub(super) async fn pump_sync_auth_worker<T: Send + 'static>(
        weak: gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        rx: mpsc::Receiver<Result<T, String>>,
        finish: impl Fn(&mut Self, Result<T, String>, &mut Context<Self>) + Clone + 'static,
    ) {
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    let finish = finish.clone();
                    let _ = weak.update(cx, move |app, cx| finish(app, result, cx));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    cx.background_executor()
                        .timer(StdDuration::from_millis(100))
                        .await;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let finish = finish.clone();
                    let _ = weak.update(cx, move |app, cx| {
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
        cancel_token: &Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) {
        // A relaunched sign-in supersedes the old flow; ignore the stale result so
        // it can't overwrite the live flow's status or a fresh session.
        if !self.finish_sync_sign_in_flow(cancel_token) {
            return;
        }
        match result {
            Ok(account) => {
                let advance_onboarding = self.sync_advance_onboarding_on_success;
                self.sync_advance_onboarding_on_success = false;
                self.settings.sync_account = Some(account);
                self.sync_auth_status = SyncAuthStatus::Idle;
                if advance_onboarding && self.show_onboarding {
                    // The account prompt is the last onboarding step, so a
                    // successful sign-in completes onboarding.
                    self.show_onboarding = false;
                    self.settings.onboarding_completed = true;
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
fn run_browser_sign_in(
    api_base: &str,
    mode: SyncAuthMode,
    cancel_token: &AtomicBool,
) -> Result<SyncAccountSettings> {
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
        Some(cancel_token),
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
    format!("{}/signin.html?{query}", sync_web_base(api_base))
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

pub(super) fn sync_account_settings_from_session(
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
