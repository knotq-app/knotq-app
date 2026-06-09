//! Account & subscription management UI for the sync account. This lives in
//! Settings → Sync (the sign-in modal stays focused on connecting); the popover
//! and modal route here via "Manage account".

use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, FontWeight, IntoElement, Window};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, Sizable};
use knotq_model::SyncAccountSettings;

use crate::app::{KnotQApp, SyncAccountAction, SyncAuthStatus};
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

impl KnotQApp {
    /// Settings → Sync body. Signed-in accounts start with a compact summary and
    /// a single Manage affordance; account and billing actions live one layer down.
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
            _ => signed_in_account_actions(
                supports_sync,
                self.sync_account_manage_open,
                in_progress,
                t,
                cx,
            ),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(sync_account_summary(&account, t))
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

fn signed_in_account_actions(
    supports_sync: bool,
    manage_open: bool,
    in_progress: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let mut panel = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(manage_account_button(manage_open, t, cx));

    if manage_open {
        let mut actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .child(check_account_status_button(in_progress, t, cx));

        if supports_sync {
            actions = actions.child(account_action_trigger(
                "sync-cancel-subscription",
                Some("Cancel sync"),
                "Cancel sync subscription",
                IconName::CircleX,
                SyncAccountAction::CancelSubscription,
                false,
                t,
                cx,
            ));
        } else {
            actions = actions.child(subscribe_button(t, cx));
        }

        panel = panel
            .child(
                actions
                    .child(online_account_button(t, cx))
                    .child(sign_out_button(t, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .p(px(8.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(token_rgba(if t.is_dark { 0xff5a5338 } else { 0xd6484030 }))
                    .bg(token_rgba(if t.is_dark { 0xff5a5314 } else { 0xff5a530c }))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(token_hsla(t.text_primary))
                                    .child("Delete account"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .line_height(px(14.0))
                                    .text_color(token_hsla(t.text_soft))
                                    .child("Account deletion is handled on knotq.com."),
                            ),
                    )
                    .child(delete_online_button(t, cx)),
            );
    }

    panel.into_any_element()
}

fn manage_account_button(is_open: bool, t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        "sync-manage-account",
        Some(if is_open { "Hide" } else { "Manage" }),
        if is_open {
            "Hide account actions"
        } else {
            "Manage sync account"
        },
        if is_open {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        },
        false,
        false,
        false,
        t,
        cx,
        |this, _window, cx| {
            this.sync_account_manage_open = !this.sync_account_manage_open;
            this.sync_account_action = None;
            cx.notify();
        },
    )
}

fn online_account_button(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        "sync-online-account",
        Some("Account page"),
        "Open account management on knotq.com",
        IconName::ExternalLink,
        false,
        false,
        false,
        t,
        cx,
        |this, _window, cx| {
            this.open_online_account_management(cx);
        },
    )
}

fn delete_online_button(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        "sync-delete-online",
        Some("Open page"),
        "Open account deletion on knotq.com",
        IconName::ExternalLink,
        false,
        true,
        false,
        t,
        cx,
        |this, _window, cx| {
            this.open_online_account_management(cx);
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
        Some("Refresh"),
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

fn sync_account_summary(account: &SyncAccountSettings, t: Theme) -> gpui::AnyElement {
    let (icon, icon_color, title, detail) = if account.supports_sync {
        (
            IconName::CircleCheck,
            if t.is_dark { 0x9af0b6ff } else { 0x176b38ff },
            "Sync is on",
            "Notes and notifications sync automatically across your devices.",
        )
    } else {
        (
            IconName::Info,
            if t.is_dark { 0xf8d38dff } else { 0x9a4b00ff },
            "Sync is not enabled",
            "Subscribe to sync notes and notifications across devices.",
        )
    };

    div()
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(
            Icon::new(icon)
                .with_size(px(14.0))
                .text_color(token_hsla(icon_color)),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(15.0))
                        .text_color(token_hsla(t.text_soft))
                        .child(detail),
                ),
        )
        .into_any_element()
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
