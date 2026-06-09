//! Account & subscription management UI for the sync account. This lives in
//! Settings → Sync (the sign-in modal stays focused on connecting); the popover
//! and modal route here via "Manage account".

use chrono::{DateTime, Local, Utc};
use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, FontWeight, IntoElement, Window};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, Sizable};
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
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(6.0))
                                .child(check_account_status_button(in_progress, t, cx))
                                .child(account_action_trigger(
                                    "sync-cancel-subscription",
                                    Some("Cancel"),
                                    "Cancel subscription",
                                    IconName::CircleX,
                                    SyncAccountAction::CancelSubscription,
                                    false,
                                    t,
                                    cx,
                                )),
                        )
                        .child(account_footer_actions(t, cx)),
                )
                .into_any_element(),
            // Signed in but without sync: one Subscribe CTA with the account footer
            // (Delete / Sign out) on the same row to stay compact. Entitlement is
            // re-checked automatically after checkout (see
            // start_subscription_status_poll), so there is no manual "I've
            // subscribed" button.
            None => div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(subscribe_button(t, cx))
                .child(account_footer_actions(t, cx))
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .children(account_status_panel(&account, t))
            .child(body)
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
        .gap(px(6.0))
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
        .child(
            Icon::new(IconName::User)
                .with_size(px(13.0))
                .text_color(token_hsla(0xffffffff)),
        )
        .child("Sign in")
        .into_any_element()
}

/// The shared bottom row for a signed-in account: the destructive "Delete
/// account" next to "Sign out", right-aligned so it reads as a footer.
fn account_footer_actions(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap(px(6.0))
        .child(account_action_trigger(
            "sync-delete-account",
            None,
            "Delete account",
            IconName::Delete,
            SyncAccountAction::DeleteAccount,
            true,
            t,
            cx,
        ))
        .child(sign_out_button(t, cx))
        .into_any_element()
}

fn sign_out_button(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        "sync-sign-out",
        Some("Sign out"),
        "Sign out",
        IconName::User,
        false,
        false,
        false,
        t,
        cx,
        |this, _window, cx| {
            this.sign_out_sync_account(cx);
        },
    )
}

/// A button that arms (but does not yet perform) a destructive account action;
/// the actual call only happens after the confirmation row's "confirm" button.
fn account_action_trigger(
    id: &'static str,
    label: Option<&'static str>,
    tooltip: &'static str,
    icon: IconName,
    action: SyncAccountAction,
    destructive: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    account_icon_button(
        id,
        label,
        tooltip,
        icon,
        false,
        destructive,
        false,
        t,
        cx,
        move |this, _window, cx| {
            this.prompt_sync_account_action(action, cx);
        },
    )
}

/// Primary CTA shown when an account has no sync entitlement: opens the hosted
/// subscription checkout in the browser.
fn subscribe_button(_t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        "sync-subscribe",
        Some("Subscribe"),
        "Subscribe to enable sync",
        IconName::ExternalLink,
        true,
        false,
        false,
        _t,
        cx,
        |this, _window, cx| {
            this.open_subscription_checkout(cx);
        },
    )
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
    account_icon_button(
        "sync-check-account-status",
        None,
        if in_progress {
            "Refreshing account status"
        } else {
            "Check account status"
        },
        if in_progress {
            IconName::LoaderCircle
        } else {
            IconName::Redo2
        },
        false,
        false,
        in_progress,
        t,
        cx,
        |this, _window, cx| {
            this.refresh_account_status(cx);
        },
    )
}

fn account_icon_button<F>(
    id: &'static str,
    label: Option<&'static str>,
    tooltip: &'static str,
    icon: IconName,
    primary: bool,
    destructive: bool,
    disabled: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut KnotQApp, &mut Window, &mut Context<KnotQApp>) + 'static,
{
    let fg = if primary {
        0xffffffff
    } else if destructive {
        0xff5a53ff
    } else {
        t.text_primary
    };
    let bg = if primary { sync_cta_bg() } else { t.button_bg };
    let hover_bg = if primary {
        sync_cta_hover_bg()
    } else {
        t.button_hover
    };
    let border = if primary {
        sync_cta_bg()
    } else {
        t.border_main
    };

    let button = div()
        .id(id)
        .h(px(28.0))
        .when(label.is_none(), |s| s.w(px(28.0)).justify_center())
        .when(label.is_some(), |s| s.px(px(9.0)))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(token_rgba(border))
        .bg(token_rgba(bg))
        .text_size(px(12.0))
        .font_weight(if primary {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(token_hsla(fg))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .when(!disabled, |s| {
            s.cursor_pointer()
                .hover(move |s| s.bg(token_rgba(hover_bg)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    on_click(this, window, cx);
                }))
        })
        .when(disabled, |s| s.opacity(0.62))
        .child(
            Icon::new(icon)
                .with_size(px(13.0))
                .text_color(token_hsla(fg)),
        );

    if let Some(label) = label {
        button
            .child(
                div()
                    .whitespace_nowrap()
                    .text_color(token_hsla(fg))
                    .child(label),
            )
            .into_any_element()
    } else {
        button.into_any_element()
    }
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
        "Free".to_string()
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
    let mut chips = vec![
        account_status_chip(
            "sync-account-plan-chip",
            IconName::Star,
            plan.clone(),
            format!("Plan: {plan}"),
            t,
        ),
        account_status_chip(
            "sync-account-subscription-chip",
            if status.subscribed {
                IconName::CircleCheck
            } else {
                IconName::CircleX
            },
            subscribed.clone(),
            format!("Subscription: {subscribed}"),
            t,
        ),
    ];
    if let Some(value) = subscription_status {
        chips.push(account_status_chip(
            "sync-account-status-chip",
            IconName::Info,
            value.clone(),
            format!("Status: {value}"),
            t,
        ));
    }
    if let Some(value) = subscription_provider {
        let label = account_level_label(&value);
        chips.push(account_status_chip(
            "sync-account-provider-chip",
            IconName::Building2,
            label.clone(),
            format!("Provider: {label}"),
            t,
        ));
    }
    if let Some(value) = current_period_end {
        chips.push(account_status_chip(
            "sync-account-period-chip",
            IconName::Calendar,
            value.clone(),
            format!("Current period ends: {value}"),
            t,
        ));
    }
    if let Some(value) = checked_at {
        chips.push(account_status_chip(
            "sync-account-checked-chip",
            IconName::Redo2,
            value.clone(),
            format!("Checked: {value}"),
            t,
        ));
    }

    Some(
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(chips)
            .into_any_element(),
    )
}

fn account_status_chip(
    id: &'static str,
    icon: IconName,
    value: String,
    tooltip: String,
    t: Theme,
) -> gpui::AnyElement {
    div()
        .id(id)
        .h(px(26.0))
        .max_w(px(180.0))
        .px(px(7.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(99.0))
        .border_1()
        .border_color(token_rgba(t.border_overlay))
        .bg(token_rgba(t.button_bg))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .child(
            Icon::new(icon)
                .with_size(px(12.0))
                .text_color(token_hsla(t.text_soft)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(11.0))
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
