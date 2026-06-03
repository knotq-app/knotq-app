//! Account & subscription management UI for the sync account. This lives in
//! Settings → Sync (the sign-in modal stays focused on connecting); the popover
//! and modal route here via "Manage account".

use chrono::{DateTime, Local, Utc};
use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, FontWeight, IntoElement};
use knotq_model::SyncAccountSettings;

use crate::app::{KnotQApp, SyncAccountAction, SyncAuthStatus};
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

impl KnotQApp {
    /// The full Settings → Sync body: status + entitlement-aware actions when
    /// signed in (with the inline destructive-action confirmation), or a sign-in
    /// entry when signed out.
    pub(crate) fn sync_account_management_section(
        &mut self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(account) = self.settings.sync_account.clone() else {
            return signed_out_entry(t, cx);
        };
        let in_progress = matches!(self.sync_auth_status, SyncAuthStatus::InProgress);
        let supports_sync = account.supports_sync;
        let armed = self.sync_account_action.is_some();

        let body: gpui::AnyElement = match self.sync_account_action {
            Some(SyncAccountAction::DeleteAccount) => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(17.0))
                        .text_color(token_hsla(0xff5a53ff))
                        .child(
                            "Delete your account and synced data? You have 14 days to undo \
                             this by signing back in before it is permanently erased.",
                        ),
                )
                .child(account_confirm_actions(
                    "Delete account",
                    in_progress,
                    t,
                    cx,
                ))
                .into_any_element(),
            Some(SyncAccountAction::CancelSubscription) => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(17.0))
                        .text_color(token_hsla(t.text_soft))
                        .child(
                            "Turn off sync for this account? Your local workspace stays on \
                             this device, and you can sign in again later to re-enable sync.",
                        ),
                )
                .child(account_confirm_actions("Turn off sync", in_progress, t, cx))
                .into_any_element(),
            None if supports_sync => div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(check_account_status_button(in_progress, t, cx))
                .child(account_action_trigger(
                    "sync-cancel-subscription",
                    "Cancel subscription",
                    SyncAccountAction::CancelSubscription,
                    false,
                    t,
                    cx,
                ))
                .child(account_action_trigger(
                    "sync-delete-account",
                    "Delete account",
                    SyncAccountAction::DeleteAccount,
                    true,
                    t,
                    cx,
                ))
                .into_any_element(),
            // Signed in but no sync entitlement: offer the paywall (subscribe +
            // re-check) above the destructive Delete action.
            None => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(15.0))
                        .text_color(token_hsla(t.text_soft))
                        .child("Sync is turned off for this account. Subscribe to enable it."),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(subscribe_button(t, cx))
                        .child(refresh_status_button(in_progress, t, cx)),
                )
                .child(div().flex().justify_end().child(account_action_trigger(
                    "sync-delete-account",
                    "Delete account",
                    SyncAccountAction::DeleteAccount,
                    true,
                    t,
                    cx,
                )))
                .into_any_element(),
        };

        div()
            .px(px(8.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(token_rgba(t.divider_tiny))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(account_status_panel(&account, t))
            .child(body)
            .when(!armed, |s| s.child(sign_out_row(t, cx)))
            .into_any_element()
    }
}

/// The signed-out state: a short prompt plus a button that opens the sign-in modal.
fn signed_out_entry(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    div()
        .px(px(8.0))
        .py(px(8.0))
        .min_h(px(38.0))
        .border_b_1()
        .border_color(token_rgba(t.divider_tiny))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child("Not signed in"),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(14.0))
                        .text_color(token_hsla(t.text_soft))
                        .child("Sign in to sync this workspace across devices."),
                ),
        )
        .child(
            div()
                .id("sync-settings-sign-in")
                .flex_shrink_0()
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(5.0))
                .bg(token_rgba(t.text_highlight))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(token_hsla(0xffffffff))
                .cursor_pointer()
                .hover(|s| s.bg(token_rgba(0xe66f1fff)))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_sync_sign_in(window, cx);
                }))
                .child("Sign in"),
        )
        .into_any_element()
}

fn sign_out_row(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    div()
        .flex()
        .justify_end()
        .child(
            div()
                .id("sync-sign-out")
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(5.0))
                .bg(token_rgba(t.button_bg))
                .text_size(px(12.0))
                .text_color(token_hsla(t.text_primary))
                .cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.sign_out_sync_account(cx);
                }))
                .child("Sign out"),
        )
        .into_any_element()
}

/// A button that arms (but does not yet perform) a destructive account action;
/// the actual call only happens after the confirmation row's "confirm" button.
fn account_action_trigger(
    id: &'static str,
    label: &'static str,
    action: SyncAccountAction,
    destructive: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id(id)
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .bg(token_rgba(t.button_bg))
        .text_size(px(12.0))
        .text_color(token_hsla(if destructive {
            0xff5a53ff
        } else {
            t.text_primary
        }))
        .cursor_pointer()
        .hover({
            let c = t.button_hover;
            move |s| s.bg(token_rgba(c))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.prompt_sync_account_action(action, cx);
        }))
        .child(label)
        .into_any_element()
}

/// Primary CTA shown when an account has no sync entitlement: opens the hosted
/// subscription checkout in the browser.
fn subscribe_button(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    div()
        .id("sync-subscribe")
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .bg(token_rgba(t.text_highlight))
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(token_hsla(0xffffffff))
        .cursor_pointer()
        .hover(|s| s.bg(token_rgba(0xe66f1fff)))
        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.open_subscription_checkout(cx);
        }))
        .child("Subscribe to enable sync")
        .into_any_element()
}

fn check_account_status_button(
    in_progress: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id("sync-check-account-status")
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .bg(token_rgba(t.button_bg))
        .text_size(px(12.0))
        .text_color(token_hsla(t.text_primary))
        .when(!in_progress, |s| {
            s.cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.refresh_account_status(cx);
                }))
        })
        .when(in_progress, |s| s.opacity(0.65))
        .child(if in_progress {
            "Checking..."
        } else {
            "Check status"
        })
        .into_any_element()
}

/// Secondary CTA next to Subscribe: re-checks entitlement after the user returns
/// from the checkout (the subscription is granted by a server-side webhook).
fn refresh_status_button(
    in_progress: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id("sync-refresh-status")
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .bg(token_rgba(t.button_bg))
        .text_size(px(12.0))
        .text_color(token_hsla(t.text_primary))
        .when(!in_progress, |s| {
            s.cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.refresh_subscription_status(cx);
                }))
        })
        .when(in_progress, |s| s.opacity(0.65))
        .child(if in_progress {
            "Checking..."
        } else {
            "I've subscribed"
        })
        .into_any_element()
}

fn account_status_panel(account: &SyncAccountSettings, t: Theme) -> gpui::AnyElement {
    let status = account.account_status.as_ref();
    let level = status
        .map(|status| account_level_label(&status.level))
        .unwrap_or_else(|| "Unknown".to_string());
    let subscribed = status
        .map(|status| {
            if status.subscribed {
                "Subscribed".to_string()
            } else {
                "Not subscribed".to_string()
            }
        })
        .unwrap_or_else(|| "Unknown".to_string());
    let sync_access = if status
        .map(|status| status.supports_sync)
        .unwrap_or(account.supports_sync)
    {
        "Enabled"
    } else {
        "Off"
    }
    .to_string();
    let subscription_status = status
        .and_then(|status| status.subscription_status.as_deref())
        .map(account_level_label);
    let subscription_provider = status
        .and_then(|status| status.subscription_provider.clone())
        .filter(|provider| !provider.trim().is_empty());
    let current_period_end =
        status.and_then(|status| status.current_period_end.map(status_date_label));
    let checked_at = status.and_then(|status| status.checked_at.map(status_timestamp_label));

    div()
        .rounded(px(6.0))
        .border_1()
        .border_color(token_rgba(t.border_overlay))
        .bg(token_rgba(t.button_bg))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(token_hsla(t.text_soft))
                .child(format!("Signed in as {}", account.email)),
        )
        .child(account_status_line("Level", level, t))
        .child(account_status_line("Subscription", subscribed, t))
        .child(account_status_line("Sync access", sync_access, t))
        .when_some(subscription_status, |s, value| {
            s.child(account_status_line("Status", value, t))
        })
        .when_some(subscription_provider, |s, value| {
            s.child(account_status_line("Provider", value, t))
        })
        .when_some(current_period_end, |s, value| {
            s.child(account_status_line("Current period ends", value, t))
        })
        .when_some(checked_at, |s, value| {
            s.child(account_status_line("Checked", value, t))
        })
        .into_any_element()
}

fn account_status_line(label: &'static str, value: String, t: Theme) -> gpui::AnyElement {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(11.0))
                .line_height(px(15.0))
                .text_color(token_hsla(t.text_soft))
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .text_size(px(11.0))
                .line_height(px(15.0))
                .text_color(token_hsla(t.text_primary))
                .child(value),
        )
        .into_any_element()
}

pub(crate) fn account_level_label(value: &str) -> String {
    let normalized = value.trim();
    if normalized.is_empty() {
        return "Free".to_string();
    }
    let mut label = normalized.replace(['_', '-'], " ");
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    label
}

fn status_date_label(value: DateTime<Utc>) -> String {
    value.with_timezone(&Local).format("%Y-%m-%d").to_string()
}

fn status_timestamp_label(value: DateTime<Utc>) -> String {
    value
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// The "are you sure?" row shown once a destructive account action is armed:
/// "Keep" backs out, the confirm button performs the call.
fn account_confirm_actions(
    confirm_label: &'static str,
    in_progress: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(8.0))
        .child(
            div()
                .id("sync-account-action-keep")
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(5.0))
                .bg(token_rgba(t.button_bg))
                .text_size(px(12.0))
                .text_color(token_hsla(t.text_primary))
                .cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.dismiss_sync_account_action(cx);
                }))
                .child("Keep"),
        )
        .child(
            div()
                .id("sync-account-action-confirm")
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(5.0))
                .bg(token_rgba(0xff5a53ff))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(token_hsla(0xffffffff))
                .when(!in_progress, |s| {
                    s.cursor_pointer()
                        .hover(|s| s.bg(token_rgba(0xd64840ff)))
                        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.confirm_sync_account_action(cx);
                        }))
                })
                .when(in_progress, |s| s.opacity(0.65))
                .child(if in_progress {
                    "Working…"
                } else {
                    confirm_label
                }),
        )
        .into_any_element()
}
