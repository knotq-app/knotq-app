use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, Entity, FontWeight, IntoElement, SharedString, Window};
use gpui_component::input::{Input, InputState};
use gpui_component::Sizable as _;
use knotq_storage_json::CalendarViewMode;

use crate::app::{
    KnotQApp, OnboardingPhase, SyncAccountAction, SyncAuthMode, SyncAuthStatus, View,
};
use crate::theme_gpui::{token_hsla, token_rgba};

// ── Onboarding spotlight steps ───────────────────────────────────────────

// Layout constants (mirrored from main.rs).
const NAVIGATOR_W: f32 = 166.0;
const LEFT_PANEL_GAP: f32 = 8.0;
const UPCOMING_W: f32 = 258.0;
const TITLE_BAR_H: f32 = 38.0;
const SCRIM_COLOR: u32 = 0x000000aa;
const SPOTLIGHT_BORDER: u32 = 0xffffff30;
const SPOTLIGHT_RADIUS: f32 = 8.0;
const CARD_W: f32 = 320.0;
const CARD_ESTIMATED_H: f32 = 152.0;
const CARD_MARGIN: f32 = 12.0;

#[derive(Clone, Copy, Eq, PartialEq)]
enum OnboardingTarget {
    Welcome,
    Calendar,
    Daily,
    Scheme,
    Upcoming,
}

struct SpotlightStep {
    title: &'static str,
    body: &'static str,
    target: OnboardingTarget,
}

const STEPS: &[SpotlightStep] = &[
    SpotlightStep {
        title: "Welcome to KnotQ",
        body: "KnotQ serves as a single app for calendar events, reminders, assignments, and general purpose notes. It aims to be simple yet functional.",
        target: OnboardingTarget::Welcome,
    },
    SpotlightStep {
        title: "Calendar Editor",
        body: "Use the calendar to create events, assignments, and reminders. Click for a reminder, shift-click for an assignment, or drag for an event.",
        target: OnboardingTarget::Calendar,
    },
    SpotlightStep {
        title: "Scheme Editor",
        body: "Schemes are editable outlines for projects, notes, and plans. You can add start and end times to each line, transforming them into visible calendar items.",
        target: OnboardingTarget::Scheme,
    },
    SpotlightStep {
        title: "Daily",
        body: "Daily is a special and default scheme. You write an optimistic task list each day and cross off the ones that you complete.",
        target: OnboardingTarget::Daily,
    },
    SpotlightStep {
        title: "Upcoming",
        body: "Upcoming displays nearby events, assignments, and reminders. You can directly mark tasks completed from here.",
        target: OnboardingTarget::Upcoming,
    },
];

/// A rectangle in the viewport (pixels from top-left of window content area).
#[derive(Clone, Copy)]
struct SpotlightRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Where to place the tooltip card relative to the spotlight.
#[derive(Clone, Copy)]
enum CardSide {
    Left,
    Center,
}

fn step_spotlight(step: usize, vw: f32, vh: f32) -> (SpotlightRect, CardSide) {
    let target = STEPS
        .get(step)
        .map(|step| step.target)
        .unwrap_or(OnboardingTarget::Welcome);
    let upcoming_x = NAVIGATOR_W + LEFT_PANEL_GAP;
    let upcoming_w = UPCOMING_W;
    let main_x = NAVIGATOR_W + LEFT_PANEL_GAP + UPCOMING_W + 1.0;
    let main_w = (vw - main_x).max(100.0);
    let body_y = TITLE_BAR_H;
    let body_h = (vh - TITLE_BAR_H).max(100.0);

    match target {
        OnboardingTarget::Welcome => (
            SpotlightRect {
                x: 0.0,
                y: 0.0,
                w: vw,
                h: vh,
            },
            CardSide::Center,
        ),
        OnboardingTarget::Calendar | OnboardingTarget::Daily | OnboardingTarget::Scheme => (
            SpotlightRect {
                x: main_x,
                y: body_y,
                w: main_w,
                h: body_h,
            },
            CardSide::Left,
        ),
        OnboardingTarget::Upcoming => (
            SpotlightRect {
                x: upcoming_x,
                y: body_y,
                w: upcoming_w,
                h: body_h,
            },
            CardSide::Left,
        ),
    }
}

fn sign_in_field(
    label: &'static str,
    input: &Entity<InputState>,
    masked: bool,
    t: crate::theme_gpui::Theme,
) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_soft))
                .child(label),
        )
        .child(
            Input::new(input)
                .appearance(false)
                .bordered(true)
                .focus_bordered(true)
                .small()
                .w_full()
                .when(masked, |input| input.mask_toggle()),
        )
        .into_any_element()
}

fn sync_auth_mode_button(
    id: &'static str,
    label: &'static str,
    mode: SyncAuthMode,
    active: bool,
    t: crate::theme_gpui::Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id(id)
        .flex_1()
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .text_size(px(12.0))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(token_hsla(if active {
            t.text_primary
        } else {
            t.text_soft
        }))
        .when(active, |s| s.bg(token_rgba(t.button_hover)))
        .when(!active, |s| {
            s.cursor_pointer().hover({
                let c = t.button_bg;
                move |h| h.bg(token_rgba(c))
            })
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.set_sync_auth_mode(mode, cx);
        }))
        .child(label)
        .into_any_element()
}

/// A button that arms (but does not yet perform) a destructive account action;
/// the actual call only happens after the confirmation row's "confirm" button.
fn account_action_trigger(
    id: &'static str,
    label: &'static str,
    action: SyncAccountAction,
    destructive: bool,
    t: crate::theme_gpui::Theme,
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

/// The "are you sure?" row shown once a destructive account action is armed:
/// "Keep" backs out, the confirm button performs the call.
fn account_confirm_actions(
    confirm_label: &'static str,
    in_progress: bool,
    t: crate::theme_gpui::Theme,
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
                .child(if in_progress { "Working…" } else { confirm_label }),
        )
        .into_any_element()
}

fn onboarding_account_choice(
    id: &'static str,
    title: &'static str,
    body: &'static str,
    mode: Option<SyncAuthMode>,
    primary: bool,
    t: crate::theme_gpui::Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id(id)
        .w_full()
        .p(px(12.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(token_rgba(if primary { t.link } else { t.border_overlay }))
        .bg(token_rgba(if primary {
            t.button_hover
        } else {
            t.button_bg
        }))
        .cursor_pointer()
        .hover({
            let c = if primary { t.link } else { t.button_hover };
            move |s| s.bg(token_rgba(c))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            if let Some(mode) = mode {
                this.open_sync_sign_in_for_onboarding(mode, window, cx);
            } else {
                this.onboarding_phase = OnboardingPhase::Guide;
                this.onboarding_page = 0;
                cx.notify();
            }
        }))
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_primary))
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(token_hsla(t.text_soft))
                .child(body),
        )
        .into_any_element()
}

impl KnotQApp {
    pub(crate) fn dismiss_notice_modal(&mut self, cx: &mut Context<Self>) {
        if self.notice_modal.take().is_some() {
            cx.notify();
        }
    }

    fn set_onboarding_page(&mut self, page: usize, cx: &mut Context<Self>) {
        self.onboarding_page = page.min(STEPS.len().saturating_sub(1));
        let Some(step) = STEPS.get(self.onboarding_page) else {
            return;
        };
        self.apply_onboarding_target(step.target, cx);
    }

    fn apply_onboarding_target(&mut self, target: OnboardingTarget, cx: &mut Context<Self>) {
        match target {
            OnboardingTarget::Welcome | OnboardingTarget::Calendar | OnboardingTarget::Upcoming => {
                if self.selection.view != View::Union {
                    self.open_union();
                }
                self.calendar_view = CalendarViewMode::Week;
            }
            OnboardingTarget::Daily => {
                if self.selection.view != View::DailyQueue {
                    self.open_daily_queue(cx);
                }
            }
            OnboardingTarget::Scheme => {
                let current_regular_scheme = self
                    .selection
                    .scheme_id
                    .filter(|_| self.selection.view == View::Scheme)
                    .filter(|id| self.workspace.scheme(*id).is_some())
                    .filter(|id| !self.workspace.is_daily_queue_scheme(*id));
                if current_regular_scheme.is_none() {
                    if let Some(id) = self.first_visible_scheme_id() {
                        self.open_scheme(id, None);
                    } else {
                        self.open_union();
                        self.calendar_view = CalendarViewMode::Week;
                    }
                }
            }
        }
    }

    pub(crate) fn render_delete_confirmation(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let pending = self.pending_delete.clone()?;
        let t = self.theme();

        Some(
            div()
                .id("delete-confirm-scrim")
                .absolute()
                .inset_0()
                .bg(token_rgba(t.overlay_scrim))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.cancel_delete_confirmation(cx);
                }))
                .child(
                    div()
                        .id("delete-confirm-modal")
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
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(token_hsla(t.text_primary))
                                .child(pending.title),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(token_hsla(t.text_muted))
                                .child(pending.message),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("delete-confirm-cancel")
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
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, _window, cx| {
                                                this.cancel_delete_confirmation(cx);
                                            },
                                        ))
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("delete-confirm-delete")
                                        .px(px(10.0))
                                        .py(px(5.0))
                                        .rounded(px(5.0))
                                        .bg(token_rgba(0xff5a53ff))
                                        .text_size(px(12.0))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(token_hsla(0xffffffff))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(token_rgba(0xe64a45ff)))
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, _window, cx| {
                                                this.confirm_pending_delete(cx);
                                            },
                                        ))
                                        .child(pending.confirm_label),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn render_sync_sign_in_modal(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let state = self.sync_sign_in.as_ref()?;
        let t = self.theme();
        let signed_in = self.settings.sync_account.clone();
        // Once the password is accepted the modal flips to its second step: collect
        // the emailed 2FA code instead of the password.
        let awaiting_code = state.challenge.is_some();
        let challenge_email = state.challenge.as_ref().map(|c| c.email.clone());
        let mode = state.mode;
        let in_progress = matches!(self.sync_auth_status, SyncAuthStatus::InProgress);
        let status = match &self.sync_auth_status {
            SyncAuthStatus::Idle => None,
            SyncAuthStatus::InProgress => Some((
                if mode == SyncAuthMode::CreateAccount {
                    "Creating account..."
                } else if awaiting_code {
                    "Verifying..."
                } else {
                    "Signing in..."
                }
                .to_string(),
                false,
            )),
            SyncAuthStatus::Error(message) => Some((message.clone(), true)),
        };

        let mut actions = div().flex().items_center().justify_between().gap(px(8.0));
        if signed_in.is_some() {
            actions = actions.child(
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
            );
        } else {
            actions = actions.child(div().flex_1());
        }

        actions = actions.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(
                    div()
                        .id("sync-sign-in-cancel")
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
                            this.close_sync_sign_in(cx);
                        }))
                        .child("Cancel"),
                )
                .child(
                    div()
                        .id("sync-sign-in-submit")
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(token_rgba(if in_progress {
                            t.button_hover
                        } else {
                            t.text_highlight
                        }))
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(0xffffffff))
                        .when(!in_progress, |s| {
                            s.cursor_pointer()
                                .hover(|s| s.bg(token_rgba(0xe66f1fff)))
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.submit_sync_sign_in(cx);
                                }))
                        })
                        .when(in_progress, |s| s.opacity(0.65))
                        .child(match (mode, awaiting_code, in_progress) {
                            (SyncAuthMode::CreateAccount, _, true) => "Creating",
                            (SyncAuthMode::CreateAccount, _, false) => "Create account",
                            (SyncAuthMode::SignIn, true, true) => "Verifying",
                            (SyncAuthMode::SignIn, true, false) => "Verify",
                            (SyncAuthMode::SignIn, false, true) => "Signing in",
                            (SyncAuthMode::SignIn, false, false) => "Sign in",
                        }),
                ),
        );

        let current_account = signed_in.clone().map(|account| {
            div()
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(token_hsla(t.text_soft))
                .child(format!("Signed in as {}", account.email))
        });

        // When signed in, offer the destructive account actions (cancel sync,
        // delete account), each gated behind an inline second confirmation.
        let supports_sync = signed_in.as_ref().is_some_and(|account| account.supports_sync);
        let account_management = signed_in.as_ref().map(|_| {
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
                    .child(account_confirm_actions("Delete account", in_progress, t, cx))
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
                None => {
                    let mut row = div().flex().items_center().gap(px(8.0));
                    if supports_sync {
                        row = row.child(account_action_trigger(
                            "sync-cancel-subscription",
                            "Cancel subscription",
                            SyncAccountAction::CancelSubscription,
                            false,
                            t,
                            cx,
                        ));
                    } else {
                        row = row.child(
                            div()
                                .flex_1()
                                .text_size(px(11.0))
                                .text_color(token_hsla(t.text_soft))
                                .child("Sync is turned off for this account."),
                        );
                    }
                    row.child(account_action_trigger(
                        "sync-delete-account",
                        "Delete account",
                        SyncAccountAction::DeleteAccount,
                        true,
                        t,
                        cx,
                    ))
                    .into_any_element()
                }
            };
            div()
                .flex()
                .flex_col()
                .pt(px(10.0))
                .border_t_1()
                .border_color(token_rgba(t.border_overlay))
                .child(body)
                .into_any_element()
        });
        let show_mode_picker = signed_in.is_none() && !awaiting_code;
        let title = match (signed_in.is_some(), mode) {
            (true, _) => "Sync account",
            (false, SyncAuthMode::CreateAccount) => "Create sync account",
            (false, SyncAuthMode::SignIn) => "Sync sign in",
        };
        let detail = match mode {
            SyncAuthMode::CreateAccount => {
                "Create an account for optional workspace sync. Local work stays available either way."
            }
            SyncAuthMode::SignIn => "Connect this app to the local Cloudflare sync Worker.",
        };
        Some(
            div()
                .id("sync-sign-in-scrim")
                .absolute()
                .inset_0()
                .bg(token_rgba(t.overlay_scrim))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.close_sync_sign_in(cx);
                }))
                .child(
                    div()
                        .id("sync-sign-in-modal")
                        .w(px(380.0))
                        .bg(token_hsla(t.bg_modal))
                        .border_1()
                        .border_color(token_rgba(t.border_overlay))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .p(px(14.0))
                        .flex()
                        .flex_col()
                        .gap(px(11.0))
                        .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(token_hsla(t.text_primary))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(token_hsla(t.text_muted))
                                .child(detail),
                        )
                        .when_some(current_account, |s, account| s.child(account))
                        .when(show_mode_picker, |s| {
                            s.child(
                                div()
                                    .flex()
                                    .gap(px(3.0))
                                    .p(px(2.0))
                                    .rounded(px(7.0))
                                    .bg(token_rgba(t.button_bg))
                                    .border_1()
                                    .border_color(token_rgba(t.border_overlay))
                                    .child(sync_auth_mode_button(
                                        "sync-auth-mode-sign-in",
                                        "Sign in",
                                        SyncAuthMode::SignIn,
                                        mode == SyncAuthMode::SignIn,
                                        t,
                                        cx,
                                    ))
                                    .child(sync_auth_mode_button(
                                        "sync-auth-mode-create-account",
                                        "Create account",
                                        SyncAuthMode::CreateAccount,
                                        mode == SyncAuthMode::CreateAccount,
                                        t,
                                        cx,
                                    )),
                            )
                        })
                        .child(sign_in_field("Sync API", &state.api_input, false, t))
                        .child(sign_in_field("Email", &state.email_input, false, t))
                        .when(!awaiting_code, |s| {
                            s.child(sign_in_field("Password", &state.password_input, true, t))
                        })
                        .when(mode == SyncAuthMode::CreateAccount && !awaiting_code, |s| {
                            s.child(
                                div()
                                    .text_size(px(11.0))
                                    .line_height(px(15.0))
                                    .text_color(token_hsla(t.text_soft))
                                    .child("Use at least 12 characters."),
                            )
                        })
                        .when(awaiting_code, |s| {
                            s.child(
                                div()
                                    .text_size(px(12.0))
                                    .line_height(px(17.0))
                                    .text_color(token_hsla(t.text_soft))
                                    .child(match &challenge_email {
                                        Some(email) => {
                                            format!("Enter the code we emailed to {email}.")
                                        }
                                        None => "Enter the code we emailed you.".to_string(),
                                    }),
                            )
                            .child(sign_in_field(
                                "Code",
                                &state.code_input,
                                false,
                                t,
                            ))
                        })
                        .when_some(status, |s, (message, is_error)| {
                            s.child(
                                div()
                                    .text_size(px(12.0))
                                    .line_height(px(17.0))
                                    .text_color(token_hsla(if is_error {
                                        0xff5a53ff
                                    } else {
                                        t.text_soft
                                    }))
                                    .child(message),
                            )
                        })
                        .when_some(account_management, |s, management| s.child(management))
                        .child(actions),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn render_notice_modal(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let notice = self.notice_modal.clone()?;
        let t = self.theme();
        Some(
            div()
                .id("notice-modal-scrim")
                .absolute()
                .inset_0()
                .bg(token_rgba(t.overlay_scrim))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.dismiss_notice_modal(cx);
                }))
                .child(
                    div()
                        .id("notice-modal")
                        .w(px(360.0))
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
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(token_hsla(t.text_primary))
                                .child(notice.title),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(token_hsla(t.text_muted))
                                .child(notice.message),
                        )
                        .child(
                            div().flex().justify_end().child(
                                div()
                                    .id("notice-modal-ok")
                                    .px(px(10.0))
                                    .py(px(5.0))
                                    .rounded(px(5.0))
                                    .bg(token_rgba(t.text_highlight))
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(token_hsla(0xffffffff))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(token_rgba(0xe66f1fff)))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        this.dismiss_notice_modal(cx);
                                    }))
                                    .child(notice.button_label),
                            ),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn render_onboarding(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.show_onboarding || self.sync_sign_in.is_some() {
            return None;
        }
        let t = self.theme();
        if self.onboarding_phase == OnboardingPhase::AccountChoice {
            return Some(
                div()
                    .id("onboarding-account-overlay")
                    .absolute()
                    .inset_0()
                    .bg(token_rgba(SCRIM_COLOR))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
                    .child(
                        div()
                            .w(px(420.0))
                            .bg(token_hsla(t.bg_modal))
                            .border_1()
                            .border_color(token_rgba(t.border_overlay))
                            .rounded(px(8.0))
                            .shadow_lg()
                            .p(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(token_hsla(t.text_primary))
                                    .child("KnotQ"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .line_height(px(18.0))
                                    .text_color(token_hsla(t.text_muted))
                                    .child(
                                        "Local-first planning with optional sync. Choose how this workspace should start.",
                                    ),
                            )
                            .child(onboarding_account_choice(
                                "onboarding-account-create",
                                "Create Sync Account",
                                "Create an account and sync this workspace across devices.",
                                Some(SyncAuthMode::CreateAccount),
                                true,
                                t,
                                cx,
                            ))
                            .child(onboarding_account_choice(
                                "onboarding-account-sign-in",
                                "Sign In",
                                "Use an existing account for this workspace.",
                                Some(SyncAuthMode::SignIn),
                                false,
                                t,
                                cx,
                            ))
                            .child(onboarding_account_choice(
                                "onboarding-account-local",
                                "Local for Now",
                                "Keep everything on this device and set up sync later from Settings.",
                                None,
                                false,
                                t,
                                cx,
                            ))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .line_height(px(15.0))
                                    .text_color(token_hsla(t.text_soft))
                                    .child("You can change sync settings at any time."),
                            ),
                    )
                    .into_any_element(),
            );
        }
        let step_index = self.onboarding_page.min(STEPS.len() - 1);
        let step = &STEPS[step_index];
        let is_last = step_index == STEPS.len() - 1;

        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let (spot, card_side) = step_spotlight(step_index, vw, vh);

        // Build the scrim as 4 rectangles around the spotlight cutout.
        let scrim_top = div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(vw))
            .h(px(spot.y))
            .bg(token_rgba(SCRIM_COLOR));
        let scrim_bottom = div()
            .absolute()
            .top(px(spot.y + spot.h))
            .left_0()
            .w(px(vw))
            .h(px((vh - spot.y - spot.h).max(0.0)))
            .bg(token_rgba(SCRIM_COLOR));
        let scrim_left = div()
            .absolute()
            .top(px(spot.y))
            .left_0()
            .w(px(spot.x))
            .h(px(spot.h))
            .bg(token_rgba(SCRIM_COLOR));
        let scrim_right = div()
            .absolute()
            .top(px(spot.y))
            .left(px(spot.x + spot.w))
            .w(px((vw - spot.x - spot.w).max(0.0)))
            .h(px(spot.h))
            .bg(token_rgba(SCRIM_COLOR));
        let welcome_scrim = div().absolute().inset_0().bg(token_rgba(SCRIM_COLOR));

        // Spotlight border highlight.
        let spotlight_border = div()
            .absolute()
            .top(px(spot.y - 1.0))
            .left(px(spot.x - 1.0))
            .w(px(spot.w + 2.0))
            .h(px(spot.h + 2.0))
            .rounded(px(SPOTLIGHT_RADIUS))
            .border_1()
            .border_color(token_rgba(SPOTLIGHT_BORDER));

        // Tooltip card.
        let (card_top, card_left) = match card_side {
            CardSide::Left => (
                spot.y + (spot.h / 2.0 - 60.0).max(0.0),
                (spot.x - CARD_W - CARD_MARGIN).max(4.0),
            ),
            CardSide::Center => (
                (vh / 2.0 - CARD_ESTIMATED_H / 2.0).max(4.0),
                (vw / 2.0 - CARD_W / 2.0).max(4.0),
            ),
        };

        let step_label: SharedString =
            SharedString::from(format!("{} / {}", step_index + 1, STEPS.len()));

        let mut buttons = div().flex().items_center().justify_between();

        buttons = buttons.child(
            div()
                .text_size(px(11.0))
                .text_color(token_hsla(t.text_soft))
                .child(step_label),
        );

        let right_buttons = {
            let mut row = div().flex().gap(px(8.0));
            if step_index > 0 {
                row = row.child(
                    div()
                        .id("onboarding-back")
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
                            this.set_onboarding_page(this.onboarding_page.saturating_sub(1), cx);
                            cx.notify();
                        }))
                        .child("Back"),
                );
            }
            let next_label: SharedString = if is_last {
                "Done".into()
            } else {
                "Next".into()
            };
            row.child(
                div()
                    .id("onboarding-next")
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .bg(token_rgba(t.link))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(token_hsla(0xffffffff))
                    .cursor_pointer()
                    .hover({
                        let c = t.link_hover;
                        move |s| s.bg(token_rgba(c))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        if is_last {
                            this.show_onboarding = false;
                            this.settings.onboarding_completed = true;
                            this.save_app_settings();
                        } else {
                            this.set_onboarding_page(this.onboarding_page + 1, cx);
                        }
                        cx.notify();
                    }))
                    .child(next_label),
            )
        };
        buttons = buttons.child(right_buttons);

        let card = div()
            .absolute()
            .top(px(card_top))
            .left(px(card_left))
            .w(px(CARD_W))
            .bg(token_hsla(t.bg_modal))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .rounded(px(8.0))
            .shadow_lg()
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_primary))
                    .child(SharedString::from(step.title)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(token_hsla(t.text_primary))
                    .child(SharedString::from(step.body)),
            )
            .child(buttons);

        // Skip button at top-right corner.
        let skip = div()
            .id("onboarding-skip")
            .absolute()
            .top(px(6.0))
            .right(px(10.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .text_size(px(11.0))
            .text_color(token_hsla(t.text_soft))
            .cursor_pointer()
            .hover({
                let c = t.text_muted;
                move |s| s.text_color(token_hsla(c))
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.show_onboarding = false;
                this.settings.onboarding_completed = true;
                this.save_app_settings();
                cx.notify();
            }))
            .child("Skip");

        Some(
            div()
                .id("onboarding-overlay")
                .absolute()
                .inset_0()
                .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
                .when(step.target == OnboardingTarget::Welcome, |overlay| {
                    overlay.child(welcome_scrim)
                })
                .when(step.target != OnboardingTarget::Welcome, |overlay| {
                    overlay
                        .child(scrim_top)
                        .child(scrim_bottom)
                        .child(scrim_left)
                        .child(scrim_right)
                        .child(spotlight_border)
                })
                .child(card)
                .child(skip)
                .into_any_element(),
        )
    }
}
