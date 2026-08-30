use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement};
use knotq_l10n::t as tr;

use crate::app::{KnotQApp, SyncAuthStatus, SyncRunStatus, View};
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

use super::{STATUS_ERROR, STATUS_OK, STATUS_SYNCING};

pub(super) struct TitleSyncStatus {
    pub(super) label: String,
    pub(super) dot_color: u32,
}

/// The distinct looks the title-bar sync pill can take. See [`KnotQApp::sync_indicator`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SyncIndicator {
    Working,
    SignedOut,
    Inactive,
    Offline,
    Error,
    UpToDate,
}

impl KnotQApp {
    pub(super) fn render_title_bar_sync_control(
        &self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let auth_in_progress = matches!(self.sync_auth_status, SyncAuthStatus::InProgress);
        let sync_active = self
            .settings
            .sync_account
            .as_ref()
            .is_some_and(|account| account.supports_sync);
        let popover_open = self.sync_status_popover.is_some();

        // Signed out or signed in without a subscription (and not mid sign-in):
        // surface an "Enable sync" call to action instead of hiding the control.
        // Clicking it opens the same status popover, which carries the matching
        // sign-in / subscribe action.
        if !sync_active && !auth_in_progress {
            return Some(self.render_enable_sync_cta(t, cx));
        }

        let status = self.title_sync_status(t);
        Some(
            div()
                .id("title-sync-account")
                .h(px(26.0))
                .px(px(9.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .bg(token_rgba(if popover_open {
                    t.button_hover
                } else {
                    t.button_bg
                }))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(7.0))
                .cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                    this.toggle_sync_status_popover(event.position(), cx);
                }))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(token_hsla(t.text_dim))
                        .child(status.label),
                )
                .child(
                    div()
                        .w(px(7.0))
                        .h(px(7.0))
                        .rounded(px(4.0))
                        .bg(token_rgba(status.dot_color)),
                )
                .into_any_element(),
        )
    }

    /// The "Enable sync" pill shown when sync isn't active yet (signed out or not
    /// subscribed). Styled like the other neutral title-bar controls — an
    /// invitation, not a loud call to action. Clicking jumps to Settings, where
    /// the sync card carries the sign-in / subscribe actions.
    fn render_enable_sync_cta(&self, t: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .id("title-sync-account")
            .h(px(26.0))
            .px(px(10.0))
            .rounded(px(7.0))
            .border_1()
            .border_color(token_rgba(t.border_soft))
            .bg(token_rgba(t.button_bg))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover({
                let c = t.button_hover;
                move |s| s.bg(token_rgba(c))
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if this.selection.view != View::Settings {
                    this.open_settings(cx);
                }
                this.focus_app_root(window);
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(token_hsla(t.text_dim))
                    .child(tr("titlebar.sync.enable_sync")),
            )
            .into_any_element()
    }

    fn title_sync_status(&self, t: Theme) -> TitleSyncStatus {
        match self.sync_indicator() {
            SyncIndicator::Working => TitleSyncStatus {
                label: tr("sync.status.sync").to_string(),
                dot_color: STATUS_SYNCING,
            },
            SyncIndicator::SignedOut => TitleSyncStatus {
                label: tr("sync.status.sync").to_string(),
                dot_color: t.text_muted,
            },
            SyncIndicator::Inactive => TitleSyncStatus {
                label: tr("sync.status.sync_inactive").to_string(),
                dot_color: STATUS_ERROR,
            },
            SyncIndicator::Offline => TitleSyncStatus {
                label: tr("sync.status.offline").to_string(),
                dot_color: STATUS_SYNCING,
            },
            SyncIndicator::Error => TitleSyncStatus {
                label: tr("sync.status.sync").to_string(),
                dot_color: STATUS_ERROR,
            },
            SyncIndicator::UpToDate => TitleSyncStatus {
                label: tr("sync.status.sync").to_string(),
                dot_color: STATUS_OK,
            },
        }
    }

    /// Whether a surface showing raw per-run sync detail is on screen.
    ///
    /// The title-bar pill collapses a run down to [`SyncIndicator`], but the
    /// sync popover renders the last-synced time and pending count, and the
    /// Settings sync card keys a spinner off `Running` directly. While either is
    /// visible a run must repaint even if the pill would not change; neither is
    /// a hot path, so the cost is irrelevant there.
    pub(crate) fn shows_live_sync_detail(&self) -> bool {
        self.sync_status_popover.is_some() || self.selection.view == View::Settings
    }

    /// Everything the title-bar sync pill actually distinguishes, and nothing
    /// else.
    ///
    /// A sync run mutates `sync_run_status`, `last_synced_at` and the pending
    /// hint on every round trip, but the pill only ever shows one of these six
    /// states — and `last_synced_at` it never shows at all. Comparing this
    /// before and after a run tells the caller whether a repaint would change a
    /// single pixel, which while typing over a live socket is almost never.
    pub(crate) fn sync_indicator(&self) -> SyncIndicator {
        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return SyncIndicator::Working;
        }
        let Some(account) = self.settings.sync_account.as_ref() else {
            return SyncIndicator::SignedOut;
        };
        if !account.supports_sync {
            return SyncIndicator::Inactive;
        }
        match &self.sync_run_status {
            // Offline is a waiting state, not a failure — use the same calm
            // blue as an active sync instead of an attention-grabbing warning.
            SyncRunStatus::Error { .. } if self.sync_offline => SyncIndicator::Offline,
            SyncRunStatus::Error { .. } => SyncIndicator::Error,
            // Deliberately keyed on unsynced WORK, not on whether a request is
            // in flight. A round trip takes tens of milliseconds and happens on
            // every edit burst and every server nudge, so showing `Running`
            // strobed the dot blue→green continuously — and each toggle repaints
            // the whole window, which is exactly what starves key events while
            // typing. Keying on pending work means the dot goes blue when you
            // have something unsynced and green when it lands: one transition
            // per burst instead of one per round trip. The popover still shows
            // the live per-run state for anyone who opens it.
            _ if self.sync_pending_count() > 0 => SyncIndicator::Working,
            _ => SyncIndicator::UpToDate,
        }
    }

    /// Largest of locally-pending CRDT edits and the count reported by the last
    /// sync run, so the indicator never under-reports unsynced work.
    pub(crate) fn sync_pending_count(&self) -> usize {
        // Counted, not reconciled: this renders on every frame, and flushing
        // the store's deferred CRDT edits here would undo the deferral.
        let local_pending = self.state.unsynced_edit_count();
        let pending_from_run = match &self.sync_run_status {
            SyncRunStatus::Running { pending }
            | SyncRunStatus::Synced { pending }
            | SyncRunStatus::Error { pending, .. } => *pending,
            SyncRunStatus::Idle => 0,
        };
        local_pending.max(pending_from_run)
    }
}
