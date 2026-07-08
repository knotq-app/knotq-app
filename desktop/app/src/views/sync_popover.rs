use chrono::{DateTime, Local, Utc};
use gpui::prelude::*;
use gpui::{
    deferred, div, px, ClickEvent, Context, FontWeight, IntoElement, MouseButton, SharedString,
    Window,
};
use knotq_l10n::{t as tr, t_with as tr_with};
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

use super::title_bar::{STATUS_ERROR, STATUS_OK, STATUS_PENDING, STATUS_SYNCING};
use crate::app::{KnotQApp, SyncAuthStatus, SyncRunStatus};
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

const SYNC_POPOVER_PRIORITY: usize = 20_000;
const CARD_W: f32 = 256.0;
const CARD_H: f32 = 150.0;
const SYNC_NOW_BG: u32 = 0x2563ebff;
const SYNC_NOW_HOVER_BG: u32 = 0x1d4ed8ff;

/// What the popover should say about the current sync state.
struct SyncStatusView {
    dot_color: u32,
    headline: SharedString,
    detail: Option<SharedString>,
}

impl KnotQApp {
    pub(crate) fn render_sync_status_popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let anchor = self.sync_status_popover?;
        let t = self.theme();
        let account = self.settings.sync_account.clone();
        let signed_in = account.is_some();
        let supports_sync = account
            .as_ref()
            .is_some_and(|account| account.supports_sync);
        let status = self.sync_status_view(t);

        let viewport_width = px(f32::from(window.viewport_size().width));
        let viewport_height = px(f32::from(window.viewport_size().height));
        let left = clamped_popover_left(anchor.x - px(16.0), px(CARD_W), viewport_width);
        let top = popover_top_biased_below(anchor.y + px(12.0), px(CARD_H), viewport_height);

        let scrim = div()
            .id("sync-popover-scrim")
            .absolute()
            .inset_0()
            .bg(token_rgba(0x00000001))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_sync_status_popover(cx);
                cx.stop_propagation();
            }));

        let mut actions = div().flex().items_center().gap(px(6.0));
        if signed_in && supports_sync {
            actions = actions.child(popover_filled_button(
                "sync-popover-sync-now",
                tr("sync.action.sync_now"),
                SYNC_NOW_BG,
                SYNC_NOW_HOVER_BG,
                cx,
                |this, _window, cx| this.sync_now(cx),
            ));
        }
        if signed_in {
            actions = actions.child(popover_button(
                "sync-popover-manage",
                tr("sync.popover.manage_account"),
                false,
                t,
                cx,
                |this, window, cx| {
                    this.close_sync_status_popover(cx);
                    this.open_settings(cx);
                    this.focus_app_root(window);
                    cx.notify();
                },
            ));
        } else {
            actions = actions.child(popover_button(
                "sync-popover-sign-in",
                tr("sync.sign_in"),
                true,
                t,
                cx,
                |this, window, cx| {
                    this.close_sync_status_popover(cx);
                    this.open_sync_sign_in(window, cx);
                },
            ));
        }

        let card = div()
            .id("sync-popover-card")
            .absolute()
            .left(left)
            .top(top)
            .w(px(CARD_W))
            .bg(token_hsla(t.bg_modal))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .rounded(px(8.0))
            .shadow_lg()
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_: &ClickEvent, _w, cx| cx.stop_propagation())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_soft))
                    .child(tr("sync.popover.title")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded(px(4.0))
                            .bg(token_rgba(status.dot_color)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(token_hsla(t.text_primary))
                            .child(status.headline),
                    ),
            )
            .when_some(status.detail, |s, detail| {
                s.child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(15.0))
                        .text_color(token_hsla(t.text_muted))
                        .child(detail),
                )
            })
            .child(actions);

        let layer = div().absolute().inset_0().child(scrim).child(card);
        Some(
            deferred(layer)
                .with_priority(SYNC_POPOVER_PRIORITY)
                .into_any_element(),
        )
    }

    fn sync_status_view(&self, t: Theme) -> SyncStatusView {
        let account = self.settings.sync_account.as_ref();
        let pending = self.sync_pending_count();

        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return SyncStatusView {
                dot_color: STATUS_SYNCING,
                headline: tr("sync.status.signing_in").into(),
                detail: None,
            };
        }

        if account.is_none() {
            return SyncStatusView {
                dot_color: t.text_muted,
                headline: tr("sync.status.not_signed_in").into(),
                detail: Some(tr("sync.popover.not_signed_in_detail").into()),
            };
        }

        if !account.is_some_and(|account| account.supports_sync) {
            return SyncStatusView {
                dot_color: STATUS_ERROR,
                headline: tr("sync.status.sync_inactive").into(),
                detail: Some(tr("sync.popover.sync_inactive_detail").into()),
            };
        }

        match &self.sync_run_status {
            SyncRunStatus::Running { pending } => SyncStatusView {
                dot_color: STATUS_SYNCING,
                headline: tr("sync.status.sync").into(),
                detail: Some(if *pending > 0 {
                    tr("sync.popover.uploading").into()
                } else {
                    tr("sync.popover.looking_for_changes").into()
                }),
            },
            // Being offline isn't an error: edits keep landing locally and the
            // daemon resyncs on its own once the connection returns, so present
            // it as a waiting state instead of surfacing the transport error.
            SyncRunStatus::Error { .. } if self.sync_offline => SyncStatusView {
                dot_color: STATUS_PENDING,
                headline: tr("sync.status.offline").into(),
                detail: Some(tr("sync.popover.offline_detail").into()),
            },
            SyncRunStatus::Error { message, .. } => SyncStatusView {
                dot_color: STATUS_ERROR,
                headline: tr("sync.status.sync_error").into(),
                detail: Some(SharedString::from(if pending > 0 {
                    tr_with(
                        "sync.popover.changes_waiting",
                        &[("error", &short_error(message))],
                    )
                } else {
                    short_error(message)
                })),
            },
            _ if pending > 0 => SyncStatusView {
                dot_color: STATUS_PENDING,
                headline: tr("sync.status.sync").into(),
                detail: Some(tr("sync.popover.waiting_next_run").into()),
            },
            _ => SyncStatusView {
                dot_color: STATUS_OK,
                headline: tr("sync.status.up_to_date").into(),
                detail: self.last_synced_at.map(|at| {
                    SharedString::from(tr_with(
                        "sync.popover.last_synced",
                        &[("when", &relative_time(at))],
                    ))
                }),
            },
        }
    }
}

/// A small action button for the popover; `primary` paints the accent fill.
fn popover_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
    on_click: impl Fn(&mut KnotQApp, &mut Window, &mut Context<KnotQApp>) + 'static,
) -> gpui::AnyElement {
    if primary {
        return popover_filled_button(id, label, t.text_highlight, 0xe66f1fff, cx, on_click);
    }

    let base = div()
        .id(id)
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            on_click(this, window, cx);
        }))
        .child(label);
    base.bg(token_rgba(t.button_bg))
        .text_color(token_hsla(t.text_primary))
        .hover({
            let c = t.button_hover;
            move |s| s.bg(token_rgba(c))
        })
        .into_any_element()
}

fn popover_filled_button(
    id: &'static str,
    label: &'static str,
    bg: u32,
    hover_bg: u32,
    cx: &mut Context<KnotQApp>,
    on_click: impl Fn(&mut KnotQApp, &mut Window, &mut Context<KnotQApp>) + 'static,
) -> gpui::AnyElement {
    div()
        .id(id)
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .bg(token_rgba(bg))
        .text_color(token_hsla(0xffffffff))
        .hover(move |s| s.bg(token_rgba(hover_bg)))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            on_click(this, window, cx);
        }))
        .child(label)
        .into_any_element()
}

/// Trim a backend error to a short, single-line phrase for the popover.
fn short_error(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message).trim();
    if first.chars().count() > 80 {
        let truncated: String = first.chars().take(79).collect();
        format!("{truncated}…")
    } else {
        first.to_string()
    }
}

fn relative_time(then: DateTime<Utc>) -> String {
    let secs = (Utc::now() - then).num_seconds().max(0);
    if secs < 45 {
        tr("sync.relative_time.just_now").to_string()
    } else if secs < 3600 {
        let mins = (secs / 60).max(1).to_string();
        tr_with("sync.relative_time.minutes_ago", &[("mins", &mins)])
    } else if secs < 86_400 {
        let hours = (secs / 3600).to_string();
        tr_with("sync.relative_time.hours_ago", &[("hours", &hours)])
    } else {
        then.with_timezone(&Local).format("%b %d").to_string()
    }
}
