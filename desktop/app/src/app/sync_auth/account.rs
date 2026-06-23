use std::sync::mpsc;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use chrono::Utc;
use gpui::Context;
use knotq_model::{SyncAccountSettings, SyncAccountStatus};
use knotq_sync::AccountStatusResponse;

use super::flow::sync_account_settings_from_session;
use super::tokens::{refresh_sync_backend, RefreshError};
use super::{
    default_sync_api_base, normalize_api_base, percent_encode, sync_web_base, CheckoutResponse,
    LoginError, LoginResponse,
};
use crate::app::google_oauth::open_browser;
use crate::app::{
    EmailVerificationResend, KnotQApp, SyncAccountAction, SyncAuthStatus, SyncRunStatus,
};

/// Where store-managed subscriptions are re-enabled (auto-renew turned back on).
/// Neither the app nor our backend can flip that for Apple/Google.
const APPLE_SUBSCRIPTIONS_URL: &str = "https://apps.apple.com/account/subscriptions";
const PLAY_SUBSCRIPTIONS_URL: &str = "https://play.google.com/store/account/subscriptions";

enum AccountStatusError {
    Unauthorized,
    Transient(anyhow::Error),
}

impl KnotQApp {
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

    /// The sync API base this build talks to: the signed-in account's stored
    /// base, else the configured default (honoring the `KNOTQ_API_BASE` override).
    /// Used to pick the matching website origin and `?api=` param for hosted pages.
    fn current_sync_api_base(&self) -> String {
        let base = self
            .settings
            .sync_account
            .as_ref()
            .map(|account| account.api_base.clone())
            .unwrap_or_else(default_sync_api_base);
        normalize_api_base(&base).unwrap_or(base)
    }

    pub fn open_online_account_management(&mut self, cx: &mut Context<Self>) {
        let api_base = self.current_sync_api_base();
        let url = format!(
            "{}/account.html?api={}#signin",
            sync_web_base(&api_base),
            percent_encode(&api_base)
        );
        match open_browser(&url) {
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
        // Dismiss the confirmation prompt and the Manage menu the moment the user
        // commits; the cancel runs in the background and the card reflects the result.
        self.sync_account_action = None;
        self.settings_dropdown = None;
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
                // The cancel response only carries entitlement, not the new
                // lifecycle, so re-read account status to surface the cancelled
                // (won't-renew) state and its period end right away.
                if supports_sync {
                    self.refresh_account_status_quiet(cx);
                }
            }
            Err(message) => {
                self.sync_auth_status = SyncAuthStatus::Error(message);
            }
        }
        cx.notify();
    }

    /// Undo a pending cancellation so the subscription renews again. Web
    /// subscriptions un-cancel through our backend; Apple/Google renewals can only
    /// be turned back on in their stores, so for those we just open the store's
    /// subscription-management page.
    pub fn reenable_sync_subscription(&mut self, cx: &mut Context<Self>) {
        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return;
        }
        let Some(account) = self.settings.sync_account.clone() else {
            return;
        };
        let provider = account
            .account_status
            .as_ref()
            .and_then(|status| status.subscription_provider.as_deref());
        match provider {
            Some("apple") => {
                self.open_store_subscription_management(APPLE_SUBSCRIPTIONS_URL, cx);
                return;
            }
            Some("google") => {
                self.open_store_subscription_management(PLAY_SUBSCRIPTIONS_URL, cx);
                return;
            }
            _ => {}
        }
        self.sync_auth_status = SyncAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result =
                        resume_subscription_backend(&account).map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });
                Self::pump_sync_auth_worker(weak, cx, rx, |app, result, cx| {
                    app.finish_reenable_subscription(result, cx);
                })
                .await;
            },
        );
        self.sync_auth_task = Some(task);
        cx.notify();
    }

    fn finish_reenable_subscription(
        &mut self,
        result: Result<SyncAccountSettings, String>,
        cx: &mut Context<Self>,
    ) {
        self.sync_auth_task = None;
        match result {
            Ok(account) => {
                self.settings.sync_account = Some(account);
                self.sync_account_action = None;
                self.sync_auth_status = SyncAuthStatus::Idle;
                self.save_app_settings();
            }
            Err(message) => {
                self.sync_auth_status = SyncAuthStatus::Error(message);
            }
        }
        cx.notify();
    }

    /// Cancel a store-managed (Apple/Google) subscription. We can't cancel it for
    /// the user — Apple exposes no cancel API and we don't cancel Play server-side —
    /// so open the store's manage-subscriptions page, where cancellation happens.
    /// The change is picked up on the next status refresh.
    pub fn cancel_store_subscription(&mut self, cx: &mut Context<Self>) {
        let provider = self
            .settings
            .sync_account
            .as_ref()
            .and_then(|account| account.account_status.as_ref())
            .and_then(|status| status.subscription_provider.as_deref());
        let url = if provider == Some("google") {
            PLAY_SUBSCRIPTIONS_URL
        } else {
            APPLE_SUBSCRIPTIONS_URL
        };
        self.open_store_subscription_management(url, cx);
    }

    /// Open a store's subscription-management page (Apple/Google) so the user can
    /// turn renewal back on; the entitlement updates on the next status refresh.
    fn open_store_subscription_management(&mut self, url: &str, cx: &mut Context<Self>) {
        match open_browser(url) {
            Ok(()) => {
                self.settings_dropdown = None;
                self.sync_auth_status = SyncAuthStatus::Idle;
            }
            Err(err) => {
                self.sync_auth_status =
                    SyncAuthStatus::Error(format!("Could not open subscription management: {err}"));
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
        // Subscribing requires a confirmed email (the backend rejects it otherwise).
        // Only short-circuit on a definite "unverified"; when it's unknown, let the
        // request proceed and surface the backend's verdict.
        let known_unverified = account
            .account_status
            .as_ref()
            .map(|status| status.email_verified == Some(false))
            .unwrap_or(false);
        if known_unverified {
            self.sync_auth_status = SyncAuthStatus::Error(
                account_action_error_message("email_not_verified").to_string(),
            );
            cx.notify();
            return;
        }
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
            Ok(url) => {
                // Open the knotq.com portal (which embeds the checkout) rather than
                // the Lemon Squeezy URL directly, so payment stays on our domain.
                // Matches the configured backend so a sandbox/local build checks out
                // against the sandbox, not production.
                let api_base = self.current_sync_api_base();
                let portal_url = format!(
                    "{}/checkout.html?url={}&api={}",
                    sync_web_base(&api_base),
                    percent_encode(&url),
                    percent_encode(&api_base)
                );
                match open_browser(&portal_url) {
                    Ok(()) => {
                        self.sync_auth_status = SyncAuthStatus::Idle;
                        // The purchase finishes in the browser with no callback into
                        // the app, so quietly poll entitlement until the webhook
                        // lands and sync turns on by itself — no manual re-check.
                        self.start_subscription_status_poll(cx);
                    }
                    Err(err) => {
                        self.sync_auth_status = SyncAuthStatus::Error(format!(
                            "Could not open the checkout page: {err}"
                        ));
                    }
                }
            }
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

    /// Quietly re-check account status (fired when the Settings view opens), so an
    /// entitlement change made outside the app — e.g. subscribing on the website —
    /// shows up without the user pressing anything. Runs in the background, never
    /// touches `sync_auth_status`, and ignores failures; the next open retries.
    pub fn refresh_account_status_quiet(&mut self, cx: &mut Context<Self>) {
        if self.sync_status_quiet_task.is_some()
            || matches!(self.sync_auth_status, SyncAuthStatus::InProgress)
        {
            return;
        }
        let Some(account) = self.settings.sync_account.clone() else {
            return;
        };

        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
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

                let _ = weak.update(cx, |app, cx| {
                    app.sync_status_quiet_task = None;
                    let Some(Ok(updated)) = result else {
                        return;
                    };
                    // The user may have signed out while the check was in flight;
                    // don't resurrect the account.
                    if app.settings.sync_account.is_none() {
                        return;
                    }
                    let was_enabled = app
                        .settings
                        .sync_account
                        .as_ref()
                        .is_some_and(|account| account.supports_sync);
                    let enabled = updated.supports_sync;
                    app.settings.sync_account = Some(updated);
                    app.save_app_settings();
                    // Re-arm the verification resend against the fresh status.
                    app.email_verification_resend = EmailVerificationResend::Idle;
                    if enabled && !was_enabled {
                        app.service_bus.signal_sync();
                    }
                    cx.notify();
                });
            },
        );
        self.sync_status_quiet_task = Some(task);
    }

    /// Resend the email-verification link to the signed-in account's address. A
    /// one-shot per armed state (the button disables to "sent"); the backend also
    /// rate-limits, so a rapid second press is harmless. Failures surface in the
    /// shared account-error line.
    pub fn resend_email_verification(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.email_verification_resend,
            EmailVerificationResend::InProgress | EmailVerificationResend::Sent
        ) {
            return;
        }
        let Some(account) = self.settings.sync_account.clone() else {
            return;
        };
        self.email_verification_resend = EmailVerificationResend::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(
                        resend_email_verification_backend(&account).map_err(|err| format!("{err:#}")),
                    );
                });
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
                let _ = weak.update(cx, |app, cx| {
                    app.email_verification_resend_task = None;
                    match result {
                        Some(Ok(())) => {
                            app.email_verification_resend = EmailVerificationResend::Sent;
                        }
                        Some(Err(message)) => {
                            app.email_verification_resend = EmailVerificationResend::Idle;
                            app.sync_auth_status = SyncAuthStatus::Error(message);
                        }
                        None => {
                            app.email_verification_resend = EmailVerificationResend::Idle;
                        }
                    }
                    cx.notify();
                });
            },
        );
        self.email_verification_resend_task = Some(task);
        cx.notify();
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
                // Re-arm the verification resend against the freshly fetched status
                // (clears a stale "sent", and a now-verified account stops prompting).
                self.email_verification_resend = EmailVerificationResend::Idle;
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
}

/// Extract the backend's machine-readable error `code` from a non-2xx response body,
/// falling back to `fallback` when the body can't be parsed.
fn login_error_code(response: ureq::Response, fallback: &str) -> String {
    response
        .into_json::<LoginError>()
        .map(|error| error.code)
        .unwrap_or_else(|_| fallback.to_string())
}

/// Shared driver for the bearer-authorized entitlement POST endpoints (cancel/resume).
/// The backend rotates the session, so the rotated credentials are returned to install.
fn account_action_request(
    account: &SyncAccountSettings,
    path: &str,
    fallback_code: &str,
    parse_context: &'static str,
) -> Result<SyncAccountSettings> {
    let base = normalize_api_base(&account.api_base)?;
    let url = format!("{base}{path}");
    let response = match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .set("authorization", &format!("Bearer {}", account.bearer_token))
        .send_json(serde_json::json!({}))
    {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => {
            return Err(anyhow!(account_action_error_message(&login_error_code(
                response,
                fallback_code
            ))));
        }
        Err(error) => return Err(anyhow!("Could not reach the sync API: {error}")),
    };
    let session: LoginResponse = response.into_json().context(parse_context)?;
    sync_account_settings_from_session(base, session)
}

/// Turn off the sync entitlement for the account (keeps the account + data).
fn cancel_subscription_backend(account: &SyncAccountSettings) -> Result<SyncAccountSettings> {
    account_action_request(
        account,
        "/v1/auth/subscription/cancel",
        "cancel_failed",
        "parse sync subscription cancel response",
    )
}

/// Undo a pending cancellation for a web subscription.
fn resume_subscription_backend(account: &SyncAccountSettings) -> Result<SyncAccountSettings> {
    account_action_request(
        account,
        "/v1/auth/subscription/resume",
        "resume_failed",
        "parse sync subscription resume response",
    )
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
            return Err(anyhow!(account_action_error_message(&login_error_code(
                response,
                "checkout_failed"
            ))));
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

fn resend_email_verification_backend(account: &SyncAccountSettings) -> Result<()> {
    let base = normalize_api_base(&account.api_base)?;
    let url = format!("{base}/v1/auth/email/verify/resend");
    match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .set("authorization", &format!("Bearer {}", account.bearer_token))
        .call()
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(429, _)) => Err(anyhow!(
            "You've requested this recently — wait a minute, then try again."
        )),
        Err(ureq::Error::Status(_, response)) => Err(anyhow!(account_action_error_message(
            &login_error_code(response, "resend_failed")
        ))),
        Err(error) => Err(anyhow!("Could not reach the sync API: {error}")),
    }
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
        subscription_state: status.subscription_state.or_else(|| {
            Some(
                if status.supports_sync {
                    "active"
                } else {
                    "inactive"
                }
                .to_string(),
            )
        }),
        subscription_provider: status.subscription_provider,
        current_period_end: status.current_period_end,
        email_verified: Some(status.email_verified),
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
        "email_not_verified" => {
            "Verify your email before subscribing — check your inbox for the link."
        }
        "email_mismatch" | "forbidden" => "This checkout does not match the signed-in account.",
        "cancel_in_app_store" => {
            "Manage this App Store subscription from your Apple account subscriptions."
        }
        "resume_in_app_store" => {
            "Re-enable this App Store subscription from your Apple account subscriptions."
        }
        "resume_in_play_store" => {
            "Re-enable this subscription from your Google Play subscriptions."
        }
        "no_active_subscription" => "There is no active web subscription to change.",
        _ => "The request to the sync API failed.",
    }
}
