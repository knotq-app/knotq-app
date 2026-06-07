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
                            "Cancel the subscription for this account? Your local workspace stays \
                             on this device. Paid sync may remain available until the current \
                             billing period ends.",
                        ),
                )
                .child(account_confirm_actions(
                    "Cancel subscription",
                    in_progress,
                    t,
                    cx,
                ))
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
            .flex()
            .flex_col()
            .gap(px(10.0))
            .children(account_status_panel(&account, t))
            .child(body)
            .when(!armed, |s| s.child(sign_out_row(t, cx)))
            .into_any_element()
    }
}

/// The signed-out state: a single full-width CTA. The card header above already
/// carries the "sign in to sync across devices" message, so we don't repeat it.
fn signed_out_entry(_t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    div()
        .id("sync-settings-sign-in")
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(6.0))
        .bg(token_rgba(sync_cta_bg()))
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(token_hsla(0xffffffff))
        .cursor_pointer()
        .hover(|s| s.bg(token_rgba(sync_cta_hover_bg())))
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.open_sync_sign_in(window, cx);
        }))
        .child("Sign in")
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
fn subscribe_button(_t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    div()
        .id("sync-subscribe")
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .bg(token_rgba(sync_cta_bg()))
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(token_hsla(0xffffffff))
        .cursor_pointer()
        .hover(|s| s.bg(token_rgba(sync_cta_hover_bg())))
        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.open_subscription_checkout(cx);
        }))
        .child("Subscribe to enable sync")
        .into_any_element()
}

pub(crate) fn sync_cta_bg() -> u32 {
    0x2563ebff
}

pub(crate) fn sync_cta_hover_bg() -> u32 {
    0x1d4ed8ff
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

/// The detail box of known entitlement facts. Returns `None` until a status has
/// actually been fetched, so we never show a box full of "Unknown" rows. The
/// email lives in the card header and sync state in the badge, so neither is
/// repeated here.
fn account_status_panel(account: &SyncAccountSettings, t: Theme) -> Option<gpui::AnyElement> {
    let status = account.account_status.as_ref()?;
    let plan = account_level_label(&status.level);
    let subscribed = if status.subscribed {
        "Subscribed".to_string()
    } else {
        "Not subscribed".to_string()
    };
    let subscription_status = status
        .subscription_status
        .as_deref()
        .map(account_level_label);
    let subscription_provider = status
        .subscription_provider
        .clone()
        .filter(|provider| !provider.trim().is_empty());
    let current_period_end = status.current_period_end.map(status_date_label);
    let checked_at = status.checked_at.map(status_timestamp_label);

    Some(
        div()
            .rounded(px(6.0))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .bg(token_rgba(t.button_bg))
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(account_status_line("Plan", plan, t))
            .child(account_status_line("Subscription", subscribed, t))
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
            .into_any_element(),
    )
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
