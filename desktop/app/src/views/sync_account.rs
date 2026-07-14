//! Account & subscription management UI for the sync account. This lives in
//! Settings → Sync (the sign-in modal stays focused on connecting); the popover
//! and modal route here via "Manage account".

use gpui::prelude::*;
use gpui::{deferred, div, px, ClickEvent, Context, FontWeight, IntoElement, Window};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, Sizable};
use knotq_l10n::t as tr;

use crate::app::{
    EmailVerificationResend, KnotQApp, SettingsDropdown, SyncAccountAction, SyncAuthStatus,
    SyncRunStatus,
};
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

/// Which provider backs the subscription, used to route the cancel action: a web
/// subscription cancels through our backend; an Apple/Google one can only be
/// cancelled in its store (Apple exposes no cancel API), so we open that page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubscriptionProvider {
    Web,
    Apple,
    Google,
}

impl SubscriptionProvider {
    fn from_slug(slug: Option<&str>) -> Self {
        match slug {
            Some("apple") => SubscriptionProvider::Apple,
            Some("google") => SubscriptionProvider::Google,
            // Web, admin overrides, and unknown providers all cancel via the
            // backend, which returns a precise error if it can't.
            _ => SubscriptionProvider::Web,
        }
    }
}

impl KnotQApp {
    /// Settings → Sync body. The card header already identifies the account
    /// (email) and state (badge), so the body stays focused on short actions.
    /// Destructive actions confirm in a dedicated modal (`render_sync_account_confirm`).
    pub(crate) fn sync_account_management_section(
        &mut self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(account) = self.settings.sync_account.clone() else {
            return signed_out_entry(t, cx);
        };
        let in_progress = matches!(self.sync_auth_status, SyncAuthStatus::InProgress);
        let syncing = matches!(self.sync_run_status, SyncRunStatus::Running { .. });
        let manage_open = self.settings_dropdown == Some(SettingsDropdown::SyncAccountManage);
        // Cancelled-but-still-entitling: sync works until the period ends, but the
        // subscription won't renew, so we offer to re-enable it instead of cancel.
        let cancelled = account
            .account_status
            .as_ref()
            .map(|status| status.is_cancelled())
            .unwrap_or(false);
        let provider = SubscriptionProvider::from_slug(
            account
                .account_status
                .as_ref()
                .and_then(|status| status.subscription_provider.as_deref()),
        );
        // Subscribing is gated on a confirmed email, so when we know it's unverified
        // we disable the CTA and spell out why rather than open a checkout the
        // backend rejects. `None` (not checked yet) is left alone — the backend
        // stays the authoritative gate.
        let known_unverified = matches!(
            account
                .account_status
                .as_ref()
                .and_then(|status| status.email_verified),
            Some(false)
        );
        let needs_verification = !account.supports_sync && known_unverified;
        let resend_in_progress =
            matches!(self.email_verification_resend, EmailVerificationResend::InProgress);
        let resend_sent =
            matches!(self.email_verification_resend, EmailVerificationResend::Sent);
        // Account-action errors (cancel/re-enable/checkout) surface here, since those
        // actions dismiss their prompt immediately rather than holding it open.
        let error = match &self.sync_auth_status {
            SyncAuthStatus::Error(message) => Some(message.clone()),
            _ => None,
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(signed_in_account_actions(
                account.supports_sync,
                cancelled,
                provider,
                manage_open,
                in_progress,
                syncing,
                known_unverified,
                t,
                cx,
            ))
            .when(needs_verification, |column| {
                column.child(email_verification_notice(resend_in_progress, resend_sent, t, cx))
            })
            .when_some(error, |column, message| {
                column.child(
                    div()
                        .w_full()
                        .text_size(px(11.0))
                        .line_height(px(15.0))
                        .text_color(token_hsla(0xff5a53ff))
                        .child(message),
                )
            })
            .into_any_element()
    }

    /// Centered confirmation modal for destructive sync-account actions, in the
    /// same style as the workspace delete confirmation.
    pub(crate) fn render_sync_account_confirm(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let action = self.sync_account_action?;
        let t = self.theme();
        let in_progress = matches!(self.sync_auth_status, SyncAuthStatus::InProgress);

        let (title, message, confirm_label) = match action {
            SyncAccountAction::CancelSubscription => (
                tr("account.confirm.cancel_subscription_title"),
                tr("account.confirm.cancel_subscription_message"),
                tr("account.confirm.cancel_subscription_confirm"),
            ),
        };

        Some(
            div()
                .id("sync-account-confirm-scrim")
                .absolute()
                .inset_0()
                .bg(token_rgba(t.overlay_scrim))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.dismiss_sync_account_action(cx);
                }))
                .child(
                    div()
                        .id("sync-account-confirm-modal")
                        .w(px(340.0))
                        .bg(token_hsla(t.bg_modal))
                        .border_1()
                        .border_color(token_rgba(t.border_overlay))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .p(px(14.0))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(token_hsla(t.text_primary))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(token_hsla(t.text_muted))
                                .child(message),
                        )
                        .child(account_confirm_actions(confirm_label, in_progress, t, cx)),
                )
                .into_any_element(),
        )
    }
}

/// Shown beneath the account actions when the email is confirmed unverified:
/// explains why Subscribe is disabled and offers to resend the verification email.
/// The resend is a one-shot per state (re-armed when status refreshes); the backend
/// also rate-limits it.
fn email_verification_notice(
    resend_in_progress: bool,
    resend_sent: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let amber = if t.is_dark { 0xf8d38dff } else { 0x9a4b00ff };
    let (label, disabled): (&str, bool) = if resend_sent {
        (tr("account.verify.email_sent"), true)
    } else if resend_in_progress {
        (tr("account.verify.sending"), true)
    } else {
        (tr("account.verify.resend"), false)
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .text_size(px(11.0))
                .line_height(px(15.0))
                .text_color(token_hsla(amber))
                .child(tr("account.verify.notice")),
        )
        .child(
            div()
                .id("sync-resend-verification")
                .text_size(px(11.0))
                .line_height(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(token_hsla(if disabled {
                    t.text_muted
                } else {
                    sync_cta_bg()
                }))
                .when(!disabled, |s| {
                    s.cursor_pointer().hover(|s| s.opacity(0.85)).on_click(
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.resend_email_verification(cx);
                        }),
                    )
                })
                .child(label),
        )
        .into_any_element()
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
        .child(tr("sync.sign_in"))
        .into_any_element()
}

/// Triggers a manual sync of the workspace. Mirrors the mobile "Resync" action
/// and the sync-status popover's "Sync now"; only shown with an active
/// subscription, since there's nothing to sync without one.
fn resync_button(syncing: bool, t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        AccountIconButtonArgs {
            id: "sync-resync",
            label: Some(if syncing {
                tr("sync.action.resyncing")
            } else {
                tr("sync.action.resync")
            }),
            tooltip: tr("sync.action.sync_now"),
            icon: IconName::Redo2,
            primary: false,
            destructive: false,
            disabled: syncing,
            full_width: false,
            t,
        },
        cx,
        |this, _window, cx| {
            this.sync_now(cx);
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn signed_in_account_actions(
    supports_sync: bool,
    cancelled: bool,
    provider: SubscriptionProvider,
    manage_open: bool,
    in_progress: bool,
    syncing: bool,
    known_unverified: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    // All controls stay grouped on the right: the primary CTA ("Subscribe" without
    // an entitlement, "Re-enable" while cancelled) sits immediately left of Manage.
    div()
        .w_full()
        .relative()
        .flex()
        .items_center()
        .justify_end()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .when(!supports_sync, |s| {
                    s.child(subscribe_button(in_progress, known_unverified, t, cx))
                })
                .when(supports_sync && cancelled, |s| {
                    s.child(reenable_button(in_progress, t, cx))
                })
                // Resync only makes sense with an active subscription; sign-out
                // now lives inside the Manage menu below.
                .when(supports_sync, |s| s.child(resync_button(syncing, t, cx)))
                .child(manage_account_button(manage_open, t, cx)),
        )
        .when(manage_open, |s| {
            s.child(deferred(manage_account_menu(
                supports_sync,
                cancelled,
                provider,
                in_progress,
                known_unverified,
                t,
                cx,
            )))
        })
        .into_any_element()
}

/// Primary CTA shown when the signed-in account has no active subscription.
/// Opens the hosted checkout, same destination as the "Subscribe" menu row.
/// Disabled until the account email is verified, since the backend gates checkout
/// on a confirmed email.
fn subscribe_button(
    in_progress: bool,
    known_unverified: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    account_icon_button(
        AccountIconButtonArgs {
            id: "sync-subscribe",
            label: Some(tr("account.subscribe.label")),
            tooltip: if known_unverified {
                tr("account.subscribe.tooltip_verify_first")
            } else {
                tr("account.subscribe.tooltip")
            },
            icon: IconName::ExternalLink,
            primary: true,
            destructive: false,
            disabled: in_progress || known_unverified,
            full_width: false,
            t,
        },
        cx,
        |this, _window, cx| {
            this.open_subscription_checkout(cx);
        },
    )
}

/// Primary CTA shown when the signed-in account's subscription is cancelled but
/// still active. Undoes the cancellation (web) or opens the store's manage page
/// (Apple/Google) so it renews again.
fn reenable_button(in_progress: bool, t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        AccountIconButtonArgs {
            id: "sync-reenable",
            label: Some(tr("account.reenable.label")),
            tooltip: tr("account.reenable.tooltip"),
            icon: IconName::Redo2,
            primary: true,
            destructive: false,
            disabled: in_progress,
            full_width: false,
            t,
        },
        cx,
        |this, _window, cx| {
            this.reenable_sync_subscription(cx);
        },
    )
}

fn manage_account_button(
    manage_open: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    account_icon_button(
        AccountIconButtonArgs {
            id: "sync-manage-account",
            label: Some(tr("account.manage.label")),
            tooltip: tr("account.manage.tooltip"),
            icon: if manage_open {
                IconName::ChevronUp
            } else {
                IconName::ChevronDown
            },
            primary: false,
            destructive: false,
            disabled: false,
            full_width: false,
            t,
        },
        cx,
        |this, _window, cx| {
            this.settings_dropdown =
                if this.settings_dropdown == Some(SettingsDropdown::SyncAccountManage) {
                    None
                } else {
                    Some(SettingsDropdown::SyncAccountManage)
                };
            this.sync_account_action = None;
            cx.notify();
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn manage_account_menu(
    supports_sync: bool,
    cancelled: bool,
    provider: SubscriptionProvider,
    in_progress: bool,
    known_unverified: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let mut rows = Vec::new();

    if !supports_sync {
        rows.push(manage_menu_row(
            ManageMenuRowArgs {
                id: ("sync-manage-subscribe", 0),
                label: if known_unverified {
                    tr("account.menu.verify_to_subscribe")
                } else {
                    tr("account.subscribe.label")
                },
                icon: IconName::ExternalLink,
                destructive: false,
                // Gated on a confirmed email, matching the primary Subscribe CTA.
                disabled: in_progress || known_unverified,
                t,
            },
            cx,
            |this, _window, cx| {
                this.open_subscription_checkout(cx);
            },
        ));
    }

    rows.push(manage_menu_row(
        ManageMenuRowArgs {
            id: ("sync-manage-check-status", 0),
            label: if supports_sync {
                tr("account.menu.check_status")
            } else {
                tr("account.menu.ive_subscribed")
            },
            icon: IconName::Redo2,
            destructive: false,
            disabled: in_progress,
            t,
        },
        cx,
        |this, _window, cx| {
            this.refresh_account_status(cx);
        },
    ));

    rows.push(manage_menu_row(
        ManageMenuRowArgs {
            id: ("sync-manage-account-page", 0),
            label: tr("account.menu.manage_on_website"),
            icon: IconName::ExternalLink,
            destructive: false,
            disabled: false,
            t,
        },
        cx,
        |this, _window, cx| {
            this.open_online_account_management(cx);
        },
    ));

    rows.push(manage_menu_row(
        ManageMenuRowArgs {
            id: ("sync-manage-sign-out", 0),
            label: tr("account.menu.sign_out"),
            icon: IconName::User,
            destructive: false,
            disabled: in_progress,
            t,
        },
        cx,
        |this, _window, cx| {
            this.sign_out_sync_account(cx);
        },
    ));

    if supports_sync {
        if cancelled {
            // Already cancelled (won't renew): offer to turn renewal back on
            // rather than cancel again.
            rows.push(manage_menu_row(
                ManageMenuRowArgs {
                    id: ("sync-manage-reenable", 0),
                    label: tr("account.menu.reenable_subscription"),
                    icon: IconName::Redo2,
                    destructive: false,
                    disabled: in_progress,
                    t,
                },
                cx,
                |this, _window, cx| {
                    this.reenable_sync_subscription(cx);
                },
            ));
        } else {
            match provider {
                // App Store / Play subscriptions can't be cancelled by us (Apple
                // has no cancel API), so send the user to the store's manage page
                // directly instead of attempting a call that always fails.
                SubscriptionProvider::Apple => rows.push(manage_menu_row(
                    ManageMenuRowArgs {
                        id: ("sync-manage-cancel-store", 0),
                        label: tr("account.menu.cancel_app_store"),
                        icon: IconName::ExternalLink,
                        destructive: true,
                        disabled: in_progress,
                        t,
                    },
                    cx,
                    |this, _window, cx| {
                        this.cancel_store_subscription(cx);
                    },
                )),
                SubscriptionProvider::Google => rows.push(manage_menu_row(
                    ManageMenuRowArgs {
                        id: ("sync-manage-cancel-store", 0),
                        label: tr("account.menu.cancel_google_play"),
                        icon: IconName::ExternalLink,
                        destructive: true,
                        disabled: in_progress,
                        t,
                    },
                    cx,
                    |this, _window, cx| {
                        this.cancel_store_subscription(cx);
                    },
                )),
                SubscriptionProvider::Web => rows.push(manage_menu_row(
                    ManageMenuRowArgs {
                        id: ("sync-manage-cancel-sync", 0),
                        label: tr("account.menu.cancel_sync"),
                        icon: IconName::CircleX,
                        destructive: true,
                        disabled: in_progress,
                        t,
                    },
                    cx,
                    |this, _window, cx| {
                        this.prompt_sync_account_action(SyncAccountAction::CancelSubscription, cx);
                    },
                )),
            }
        }
    }

    div()
        .absolute()
        .top(px(34.0))
        .right_0()
        .w(px(236.0))
        .p(px(4.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(token_rgba(t.border_main))
        .bg(token_rgba(t.bg_modal))
        .shadow_md()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .children(rows)
        .into_any_element()
}

pub(crate) fn sync_cta_bg() -> u32 {
    0x2563ebff
}

pub(crate) fn sync_cta_hover_bg() -> u32 {
    0x1d4ed8ff
}

struct ManageMenuRowArgs {
    id: (&'static str, usize),
    label: &'static str,
    icon: IconName,
    destructive: bool,
    disabled: bool,
    t: Theme,
}

fn manage_menu_row<F>(
    args: ManageMenuRowArgs,
    cx: &mut Context<KnotQApp>,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut KnotQApp, &mut Window, &mut Context<KnotQApp>) + 'static,
{
    let ManageMenuRowArgs {
        id,
        label,
        icon,
        destructive,
        disabled,
        t,
    } = args;
    let fg = if destructive {
        0xff5a53ff
    } else {
        t.text_primary
    };

    div()
        .id(id)
        .w_full()
        .min_h(px(30.0))
        .px(px(8.0))
        .py(px(5.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(5.0))
        .text_color(token_hsla(fg))
        .when(!disabled, |s| {
            s.cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |h| h.bg(token_rgba(c))
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.settings_dropdown = None;
                    on_click(this, window, cx);
                }))
        })
        .when(disabled, |s| s.opacity(0.55))
        .child(
            Icon::new(icon)
                .with_size(px(14.0))
                .text_color(token_hsla(fg)),
        )
        .child(
            div()
                .min_w_0()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(token_hsla(fg))
                .child(label),
        )
        .into_any_element()
}

struct AccountIconButtonArgs {
    id: &'static str,
    label: Option<&'static str>,
    tooltip: &'static str,
    icon: IconName,
    primary: bool,
    destructive: bool,
    disabled: bool,
    full_width: bool,
    t: Theme,
}

fn account_icon_button<F>(
    args: AccountIconButtonArgs,
    cx: &mut Context<KnotQApp>,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut KnotQApp, &mut Window, &mut Context<KnotQApp>) + 'static,
{
    let AccountIconButtonArgs {
        id,
        label,
        tooltip,
        icon,
        primary,
        destructive,
        disabled,
        full_width,
        t,
    } = args;
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
        .h(px(30.0))
        .when(full_width, |s| s.w_full().justify_center())
        .when(label.is_none() && !full_width, |s| {
            s.w(px(30.0)).justify_center()
        })
        .when(label.is_some() || full_width, |s| s.px(px(8.0)))
        .flex()
        .items_center()
        .gap(px(3.0))
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
                .with_size(px(13.5))
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
                .child(tr("account.confirm.keep_subscription")),
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
                    tr("account.confirm.working")
                } else {
                    confirm_label
                }),
        )
        .into_any_element()
}
