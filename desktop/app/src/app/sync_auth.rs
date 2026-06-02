use std::sync::mpsc;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use chrono::{DateTime, Utc};
use gpui::{AppContext, Context, Window};
use gpui_component::input::InputState;
use knotq_model::{SyncAccountSettings, WorkspaceId};
use serde::{Deserialize, Serialize};

use super::{KnotQApp, PendingLoginChallenge, SyncAuthStatus, SyncRunStatus, SyncSignInState};

const DEFAULT_SYNC_API_BASE: &str = "http://127.0.0.1:8787";

#[derive(Serialize)]
struct LoginRequest {
    email: String,
    password: String,
}

/// Pending two-factor login returned by `POST /v1/auth/login`: the password was
/// accepted and a code emailed, but no session is minted until the code is verified.
#[derive(Deserialize)]
struct LoginChallengeResponse {
    challenge_id: String,
}

/// The challenge plus the context the verify step needs (normalized base + email).
pub(crate) struct LoginChallenge {
    pub api_base: String,
    pub email: String,
    pub challenge_id: String,
}

#[derive(Deserialize)]
struct LoginResponse {
    #[serde(default)]
    session_id: Option<String>,
    user_id: String,
    #[serde(default)]
    workspace_id: Option<WorkspaceId>,
    email: String,
    #[serde(default = "default_supports_sync")]
    supports_sync: bool,
    bearer_token: String,
    expires_at: DateTime<Utc>,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    refresh_expires_at: Option<DateTime<Utc>>,
}

/// Fresh credentials returned by `POST /v1/auth/refresh`.
pub(crate) struct RefreshedTokens {
    pub bearer_token: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_token: String,
    pub refresh_expires_at: Option<DateTime<Utc>>,
    pub supports_sync: bool,
}

/// Why a refresh attempt failed. `Unauthorized` means the refresh token is dead
/// (revoked/expired/replayed) and the user must sign in again; `Transient` is a
/// network/parse hiccup worth retrying on the next sync tick.
pub(crate) enum RefreshError {
    Unauthorized,
    Transient(anyhow::Error),
}

#[derive(Deserialize)]
struct LoginError {
    code: String,
}

impl KnotQApp {
    pub fn open_sync_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sync_sign_in.is_some() {
            return;
        }
        self.close_repeat_popover();
        self.cancel_event_popup_without_commit(cx);
        self.search_open = false;

        let api_base = self
            .settings
            .sync_account
            .as_ref()
            .map(|account| account.api_base.clone())
            .unwrap_or_else(|| DEFAULT_SYNC_API_BASE.to_string());
        let email = self
            .settings
            .sync_account
            .as_ref()
            .map(|account| account.email.clone())
            .unwrap_or_default();

        let api_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Sync API")
                .default_value(api_base)
        });
        let email_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Email")
                .default_value(email)
        });
        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let code_input = cx.new(|cx| InputState::new(window, cx).placeholder("6-digit code"));

        email_input.update(cx, |input, cx| input.focus(window, cx));
        self.sync_sign_in = Some(SyncSignInState {
            api_input,
            email_input,
            password_input,
            code_input,
            challenge: None,
        });
        self.sync_auth_status = SyncAuthStatus::Idle;
        cx.notify();
    }

    pub fn close_sync_sign_in(&mut self, cx: &mut Context<Self>) {
        if self.sync_sign_in.take().is_some() {
            self.sync_auth_status = SyncAuthStatus::Idle;
            cx.notify();
        }
    }

    pub fn sign_out_sync_account(&mut self, cx: &mut Context<Self>) {
        if let Some(account) = self.settings.sync_account.take() {
            std::thread::spawn(move || {
                let _ = logout_sync_backend(&account);
            });
            self.save_app_settings();
        }
        self.sync_auth_status = SyncAuthStatus::Idle;
        self.sync_run_status = SyncRunStatus::Idle;
        cx.notify();
    }

    pub fn submit_sync_sign_in(&mut self, cx: &mut Context<Self>) {
        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return;
        }
        let Some(state) = &self.sync_sign_in else {
            return;
        };

        // Second step: a challenge is pending, so the submit verifies the emailed code.
        if let Some(challenge) = &state.challenge {
            let code = state.code_input.read(cx).value().to_string();
            if code.trim().is_empty() {
                self.sync_auth_status =
                    SyncAuthStatus::Error("Enter the code we emailed you.".to_string());
                cx.notify();
                return;
            }
            let api_base = challenge.api_base.clone();
            let challenge_id = challenge.challenge_id.clone();
            let code = code.trim().to_string();
            self.sync_auth_status = SyncAuthStatus::InProgress;
            let task = cx.spawn(
                async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let result = verify_login_to_sync_backend(&api_base, &challenge_id, &code)
                            .map_err(|err| format!("{err:#}"));
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
            return;
        }

        // First step: email + password earns a 2FA challenge (a code is emailed).
        let api_base = state.api_input.read(cx).value().to_string();
        let email = state.email_input.read(cx).value().to_string();
        let password = state.password_input.read(cx).value().to_string();
        let api_base = normalize_api_base(&api_base).unwrap_or_else(|err| {
            self.sync_auth_status = SyncAuthStatus::Error(err.to_string());
            String::new()
        });
        if api_base.is_empty() {
            cx.notify();
            return;
        }
        if email.trim().is_empty() || password.is_empty() {
            self.sync_auth_status =
                SyncAuthStatus::Error("Enter the email and password for your account.".to_string());
            cx.notify();
            return;
        }

        self.sync_auth_status = SyncAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = login_to_sync_backend(&api_base, &email, &password)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });
                Self::pump_sync_auth_worker(weak, cx, rx, |app, result, cx| {
                    app.finish_sync_login(result, cx);
                })
                .await;
            },
        );
        self.sync_auth_task = Some(task);
        cx.notify();
    }

    /// Poll a background auth worker's channel from the UI executor and hand the
    /// result to `finish` on the app entity. Shared by the password and code steps.
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

    /// Completion for the password step: store the pending challenge so the modal
    /// switches to its code-entry phase (or surface the error).
    fn finish_sync_login(
        &mut self,
        result: Result<LoginChallenge, String>,
        cx: &mut Context<Self>,
    ) {
        self.sync_auth_task = None;
        match result {
            Ok(challenge) => {
                if let Some(state) = self.sync_sign_in.as_mut() {
                    state.challenge = Some(PendingLoginChallenge {
                        api_base: challenge.api_base,
                        email: challenge.email,
                        challenge_id: challenge.challenge_id,
                    });
                }
                self.sync_auth_status = SyncAuthStatus::Idle;
            }
            Err(message) => {
                self.sync_auth_status = SyncAuthStatus::Error(message);
            }
        }
        cx.notify();
    }

    fn finish_sync_sign_in(
        &mut self,
        result: Result<SyncAccountSettings, String>,
        cx: &mut Context<Self>,
    ) {
        self.sync_auth_task = None;
        match result {
            Ok(account) => {
                self.settings.sync_account = Some(account);
                self.sync_sign_in = None;
                self.sync_auth_status = SyncAuthStatus::Idle;
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

/// First login step: submit email + password. On success the backend emails a
/// 2FA code and returns a challenge to be completed via `verify_login_to_sync_backend`.
fn login_to_sync_backend(api_base: &str, email: &str, password: &str) -> Result<LoginChallenge> {
    let base = normalize_api_base(api_base)?;
    let email = email.trim().to_string();
    let url = format!("{base}/v1/auth/login");
    let request = LoginRequest {
        email: email.clone(),
        password: password.to_string(),
    };
    let response = match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .send_json(serde_json::to_value(request)?)
    {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => {
            let code = response
                .into_json::<LoginError>()
                .map(|error| error.code)
                .unwrap_or_else(|_| "unauthorized".to_string());
            return Err(anyhow!(login_error_message(&code)));
        }
        Err(error) => return Err(anyhow!("Could not reach the local sync Worker: {error}")),
    };
    let challenge: LoginChallengeResponse = response
        .into_json()
        .context("parse sync login challenge from local backend")?;
    Ok(LoginChallenge {
        api_base: base,
        email,
        challenge_id: challenge.challenge_id,
    })
}

/// Second login step: submit the emailed code for a challenge and, on success,
/// receive the session. Runs on a background thread (blocking `ureq`).
fn verify_login_to_sync_backend(
    api_base: &str,
    challenge_id: &str,
    code: &str,
) -> Result<SyncAccountSettings> {
    let base = normalize_api_base(api_base)?;
    let url = format!("{base}/v1/auth/login/verify");
    let response = match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .send_json(serde_json::json!({ "challenge_id": challenge_id, "code": code.trim() }))
    {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => {
            let code = response
                .into_json::<LoginError>()
                .map(|error| error.code)
                .unwrap_or_else(|_| "unauthorized".to_string());
            return Err(anyhow!(login_error_message(&code)));
        }
        Err(error) => return Err(anyhow!("Could not reach the local sync Worker: {error}")),
    };
    let session: LoginResponse = response
        .into_json()
        .context("parse sync login verify response from local backend")?;
    Ok(SyncAccountSettings {
        api_base: base,
        user_id: session.user_id,
        session_id: session.session_id,
        workspace_id: session.workspace_id,
        email: session.email,
        supports_sync: session.supports_sync,
        bearer_token: session.bearer_token,
        expires_at: session.expires_at,
        refresh_token: non_empty(session.refresh_token),
        refresh_expires_at: session.refresh_expires_at,
    })
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
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

fn default_supports_sync() -> bool {
    true
}

fn normalize_api_base(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!("Enter a sync API URL."));
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(anyhow!("Sync API must start with http:// or https://."));
    }
    Ok(match trimmed {
        "http://127.0.0.1:7878" | "http://localhost:7878" => DEFAULT_SYNC_API_BASE.to_string(),
        _ => trimmed.to_string(),
    })
}

fn login_error_message(code: &str) -> &'static str {
    match code {
        "invalid_email" | "unauthorized" => "Email or password is incorrect.",
        "password_too_long" => "Password is too long.",
        "invalid_code" => "That code is incorrect.",
        "code_expired" | "invalid_or_expired_code" => {
            "That code has expired. Sign in again to get a new one."
        }
        "too_many_attempts" => "Too many incorrect codes. Sign in again to get a new one.",
        _ => "Sign in failed.",
    }
}
