use std::sync::mpsc;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use chrono::{DateTime, Utc};
use gpui::{AppContext, Context, Window};
use gpui_component::input::InputState;
use knotq_model::{SyncAccountSettings, WorkspaceId};
use serde::{Deserialize, Serialize};

use super::{KnotQApp, SyncAuthStatus, SyncRunStatus, SyncSignInState};

const DEFAULT_SYNC_API_BASE: &str = "http://127.0.0.1:8787";

#[derive(Serialize)]
struct LoginRequest {
    email: String,
    password: String,
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

        email_input.update(cx, |input, cx| input.focus(window, cx));
        self.sync_sign_in = Some(SyncSignInState {
            api_input,
            email_input,
            password_input,
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

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_sync_sign_in(result, cx);
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
                                app.finish_sync_sign_in(
                                    Err("Sync sign-in worker stopped".to_string()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.sync_auth_task = Some(task);
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

fn login_to_sync_backend(
    api_base: &str,
    email: &str,
    password: &str,
) -> Result<SyncAccountSettings> {
    let url = format!("{}/v1/auth/login", normalize_api_base(api_base)?);
    let request = LoginRequest {
        email: email.trim().to_string(),
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
    let session: LoginResponse = response
        .into_json()
        .context("parse sync login response from local backend")?;
    Ok(SyncAccountSettings {
        api_base: normalize_api_base(api_base)?,
        user_id: session.user_id,
        session_id: session.session_id,
        workspace_id: session.workspace_id,
        email: session.email,
        supports_sync: session.supports_sync,
        bearer_token: session.bearer_token,
        expires_at: session.expires_at,
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
        _ => "Sign in failed.",
    }
}
