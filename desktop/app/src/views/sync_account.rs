//! Account & subscription management UI for the sync account. This lives in
//! Settings → Sync (the sign-in modal stays focused on connecting); the popover
//! and modal route here via "Manage account".

use gpui::prelude::*;
use gpui::{deferred, div, px, ClickEvent, Context, FontWeight, IntoElement, Window};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, Sizable};

use crate::app::{KnotQApp, SettingsDropdown, SyncAccountAction, SyncAuthStatus, SyncRunStatus};
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
                t,
                cx,
            ))
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
                "Cancel subscription?",
                "Sync stays available until the current billing period ends, and your \
                 workspace stays on this device.",
                "Cancel subscription",
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

/// Triggers a manual sync of the workspace. Mirrors the mobile "Resync" action
/// and the sync-status popover's "Sync now"; only shown with an active
/// subscription, since there's nothing to sync without one.
fn resync_button(syncing: bool, t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        "sync-resync",
        Some(if syncing {
            "Resyncing\u{2026}"
        } else {
            "Resync"
        }),
        "Sync now",
        IconName::Redo2,
        false,
        false,
        syncing,
        false,
        t,
        cx,
        |this, _window, cx| {
            this.sync_now(cx);
        },
    )
}

fn signed_in_account_actions(
    supports_sync: bool,
    cancelled: bool,
    provider: SubscriptionProvider,
    manage_open: bool,
    in_progress: bool,
    syncing: bool,
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
                    s.child(subscribe_button(in_progress, t, cx))
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
                t,
                cx,
            )))
        })
        .into_any_element()
}

/// Primary CTA shown when the signed-in account has no active subscription.
/// Opens the hosted checkout, same destination as the "Subscribe" menu row.
fn subscribe_button(in_progress: bool, t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    account_icon_button(
        "sync-subscribe",
        Some("Subscribe"),
        "Subscribe to KnotQ Sync",
        IconName::ExternalLink,
        true,
        false,
        in_progress,
        false,
        t,
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
        "sync-reenable",
        Some("Re-enable"),
        "Re-enable your subscription so it renews",
        IconName::Redo2,
        true,
        false,
        in_progress,
        false,
        t,
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
        "sync-manage-account",
        Some("Manage"),
        "Manage sync account",
        if manage_open {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        },
        false,
        false,
        false,
        false,
        t,
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

fn manage_account_menu(
    supports_sync: bool,
    cancelled: bool,
    provider: SubscriptionProvider,
    in_progress: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let mut rows = Vec::new();

    if !supports_sync {
        rows.push(manage_menu_row(
            ("sync-manage-subscribe", 0),
            "Subscribe",
            IconName::ExternalLink,
            false,
            in_progress,
            t,
            cx,
            |this, _window, cx| {
                this.open_subscription_checkout(cx);
            },
        ));
    }

    rows.push(manage_menu_row(
        ("sync-manage-check-status", 0),
        if supports_sync {
            "Check status"
        } else {
            "I've subscribed"
        },
        IconName::Redo2,
        false,
        in_progress,
        t,
        cx,
        |this, _window, cx| {
            this.refresh_account_status(cx);
        },
    ));

    rows.push(manage_menu_row(
        ("sync-manage-account-page", 0),
        "Manage account on website",
        IconName::ExternalLink,
        false,
        false,
        t,
        cx,
        |this, _window, cx| {
            this.open_online_account_management(cx);
        },
    ));

    rows.push(manage_menu_row(
        ("sync-manage-sign-out", 0),
        "Sign out",
        IconName::User,
        false,
        in_progress,
        t,
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
                ("sync-manage-reenable", 0),
                "Re-enable subscription",
                IconName::Redo2,
                false,
                in_progress,
                t,
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
                    ("sync-manage-cancel-store", 0),
                    "Cancel in App Store",
                    IconName::ExternalLink,
                    true,
                    in_progress,
                    t,
                    cx,
                    |this, _window, cx| {
                        this.cancel_store_subscription(cx);
                    },
                )),
                SubscriptionProvider::Google => rows.push(manage_menu_row(
                    ("sync-manage-cancel-store", 0),
                    "Cancel in Google Play",
                    IconName::ExternalLink,
                    true,
                    in_progress,
                    t,
                    cx,
                    |this, _window, cx| {
                        this.cancel_store_subscription(cx);
                    },
                )),
                SubscriptionProvider::Web => rows.push(manage_menu_row(
                    ("sync-manage-cancel-sync", 0),
                    "Cancel sync",
                    IconName::CircleX,
                    true,
                    in_progress,
                    t,
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

fn manage_menu_row<F>(
    id: (&'static str, usize),
    label: &'static str,
    icon: IconName,
    destructive: bool,
    disabled: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut KnotQApp, &mut Window, &mut Context<KnotQApp>) + 'static,
{
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

fn account_icon_button<F>(
    id: &'static str,
    label: Option<&'static str>,
    tooltip: &'static str,
    icon: IconName,
    primary: bool,
    destructive: bool,
    disabled: bool,
    full_width: bool,
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
                .child("Keep subscription"),
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
