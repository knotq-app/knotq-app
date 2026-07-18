use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window};
use knotq_storage_json::CalendarViewMode;

use crate::app::{KnotQApp, View};
#[cfg(feature = "accounts")]
use crate::app::OnboardingPhase;
#[cfg(feature = "accounts")]
use crate::app::{SyncAuthMode, SyncAuthStatus};
use crate::theme_gpui::{token_hsla, token_rgba};
#[cfg(feature = "accounts")]
use crate::theme_gpui::Theme;
#[cfg(feature = "accounts")]
use crate::views::{sync_cta_bg, sync_cta_hover_bg};

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
    /// l10n key for the title.
    title: &'static str,
    /// l10n key for the body.
    body: &'static str,
    target: OnboardingTarget,
}

// Copy is kept consistent with the mobile (iOS/Android) tours; only the Calendar
// step uses desktop-specific gesture wording (click / shift-click / drag).
const STEPS: &[SpotlightStep] = &[
    SpotlightStep {
        title: "onboarding.step.welcome.title",
        body: "onboarding.step.welcome.body",
        target: OnboardingTarget::Welcome,
    },
    SpotlightStep {
        title: "onboarding.step.calendar.title",
        body: "onboarding.step.calendar.body",
        target: OnboardingTarget::Calendar,
    },
    SpotlightStep {
        title: "onboarding.step.schemes.title",
        body: "onboarding.step.schemes.body",
        target: OnboardingTarget::Scheme,
    },
    SpotlightStep {
        title: "onboarding.step.daily.title",
        body: "onboarding.step.daily.body",
        target: OnboardingTarget::Daily,
    },
    SpotlightStep {
        title: "onboarding.step.upcoming.title",
        body: "onboarding.step.upcoming.body",
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

/// Visual weight of an onboarding account button, mirroring the Settings → Sync
/// buttons: a blue primary CTA, a bordered secondary, and a quiet ghost link.
#[cfg(feature = "accounts")]
#[derive(Clone, Copy)]
enum AccountChoiceVariant {
    Primary,
    Secondary,
    Ghost,
}

/// A single onboarding account action, styled like the buttons in Settings → Sync.
/// `mode` opens the browser sign-in (Some) or continues local-only (None).
#[cfg(feature = "accounts")]
fn onboarding_account_choice(
    id: &'static str,
    label: &'static str,
    mode: Option<SyncAuthMode>,
    variant: AccountChoiceVariant,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let button = div()
        .id(id)
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .text_size(px(13.0))
        .cursor_pointer();
    let button = match variant {
        AccountChoiceVariant::Primary => button
            .bg(token_rgba(sync_cta_bg()))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(token_hsla(0xffffffff))
            .hover(|s| s.bg(token_rgba(sync_cta_hover_bg()))),
        AccountChoiceVariant::Secondary => button
            .bg(token_rgba(t.button_bg))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(token_hsla(t.text_primary))
            .hover({
                let c = t.button_hover;
                move |s| s.bg(token_rgba(c))
            }),
        AccountChoiceVariant::Ghost => button.text_color(token_hsla(t.text_primary)).hover({
            let bg = t.button_hover;
            move |s| s.bg(token_rgba(bg))
        }),
    };
    button
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            if let Some(mode) = mode {
                this.open_sync_sign_in_for_onboarding(mode, window, cx);
            } else {
                this.finish_onboarding();
                cx.notify();
            }
        }))
        .child(label)
        .into_any_element()
}

impl KnotQApp {
    pub(crate) fn dismiss_notice_modal(&mut self, cx: &mut Context<Self>) {
        if self.notice_modal.take().is_some() {
            cx.notify();
        }
    }

    /// Mark onboarding finished and persist it. Callers should `cx.notify()`.
    pub(crate) fn finish_onboarding(&mut self) {
        self.show_onboarding = false;
        self.settings.onboarding_completed = true;
        self.save_app_settings();
    }

    /// After the tutorial (via "Done" or "Skip"): surface the sign-in / stay-local
    /// prompt, unless the user is already signed in, in which case we're done.
    fn advance_past_tutorial(&mut self) {
        // Without the `accounts` feature there is no sign-in / stay-local prompt, so
        // finishing the tutorial simply completes onboarding.
        #[cfg(feature = "accounts")]
        if self.settings.sync_account.is_some() {
            self.finish_onboarding();
        } else {
            self.onboarding_phase = OnboardingPhase::AccountChoice;
        }
        #[cfg(not(feature = "accounts"))]
        self.finish_onboarding();
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
                                        .child(knotq_l10n::t("common.cancel")),
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
        if !self.show_onboarding {
            return None;
        }
        let t = self.theme();
        #[cfg(feature = "accounts")]
        if self.onboarding_phase == OnboardingPhase::AccountChoice {
            // Sign-in happens in the browser, so surface its progress/errors here
            // (there is no longer an in-app sign-in modal to host them).
            let status: Option<(String, bool)> = match &self.sync_auth_status {
                SyncAuthStatus::Idle => None,
                SyncAuthStatus::InProgress => Some((
                    knotq_l10n::t("onboarding.sync_prompt.opening_browser").to_string(),
                    false,
                )),
                SyncAuthStatus::Error(message) => Some((message.clone(), true)),
            };
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
                            .gap(px(14.0))
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(token_hsla(t.text_primary))
                                    .child(knotq_l10n::t("onboarding.sync_prompt.title")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .line_height(px(18.0))
                                    .text_color(token_hsla(t.text_soft))
                                    .child(knotq_l10n::t("onboarding.sync_prompt.body")),
                            )
                            // Settings → Sync-styled panel: a bordered card holding
                            // the primary/secondary account actions.
                            .child(
                                div()
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(token_rgba(t.border_overlay))
                                    .bg(token_rgba(t.button_bg))
                                    .p(px(12.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.0))
                                    .child(
                                        div().flex().flex_col().gap(px(2.0)).child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(token_hsla(t.text_primary))
                                                .child(knotq_l10n::t("onboarding.sync_prompt.panel_title")),
                                        ),
                                    )
                                    .child(onboarding_account_choice(
                                        "onboarding-account-create",
                                        knotq_l10n::t("onboarding.sync_prompt.sign_up"),
                                        Some(SyncAuthMode::CreateAccount),
                                        AccountChoiceVariant::Primary,
                                        t,
                                        cx,
                                    ))
                                    .child(onboarding_account_choice(
                                        "onboarding-account-sign-in",
                                        knotq_l10n::t("onboarding.sync_prompt.sign_in"),
                                        Some(SyncAuthMode::SignIn),
                                        AccountChoiceVariant::Secondary,
                                        t,
                                        cx,
                                    )),
                            )
                            .child(onboarding_account_choice(
                                "onboarding-account-local",
                                knotq_l10n::t("onboarding.sync_prompt.continue_local"),
                                None,
                                AccountChoiceVariant::Ghost,
                                t,
                                cx,
                            ))
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

        let step_label: SharedString = SharedString::from(knotq_l10n::t_with(
            "onboarding.tour.step_label",
            &[
                ("current", &(step_index + 1).to_string()),
                ("total", &STEPS.len().to_string()),
            ],
        ));

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
                        .child(knotq_l10n::t("common.back")),
                );
            }
            let next_label: SharedString = if !is_last {
                knotq_l10n::t("onboarding.tour.next").into()
            } else if self.settings.sync_account.is_some() {
                knotq_l10n::t("common.done").into()
            } else {
                knotq_l10n::t("onboarding.tour.continue").into()
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
                            this.advance_past_tutorial();
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
                    .child(SharedString::from(knotq_l10n::t(step.title))),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(token_hsla(t.text_primary))
                    .child(SharedString::from(knotq_l10n::t(step.body))),
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
                let c = t.text_primary;
                move |s| s.text_color(token_hsla(c))
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.advance_past_tutorial();
                cx.notify();
            }))
            .child(knotq_l10n::t("onboarding.tour.skip"));

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
