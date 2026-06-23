use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement};

use crate::app::{KnotQApp, SyncAuthStatus, SyncRunStatus, View};
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

use super::{STATUS_ERROR, STATUS_OK, STATUS_PENDING, STATUS_SYNCING};

pub(super) struct TitleSyncStatus {
    pub(super) label: String,
    pub(super) dot_color: u32,
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
                    .child("Enable sync"),
            )
            .into_any_element()
    }

    fn title_sync_status(&self, t: Theme) -> TitleSyncStatus {
        let account = self.settings.sync_account.as_ref();
        let pending = self.sync_pending_count();

        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return TitleSyncStatus {
                label: "Sync".to_string(),
                dot_color: STATUS_SYNCING,
            };
        }

        if account.is_none() {
            return TitleSyncStatus {
                label: "Sync".to_string(),
                dot_color: t.text_muted,
            };
        }

        if account.is_some_and(|account| !account.supports_sync) {
            return TitleSyncStatus {
                label: "Sync inactive".to_string(),
                dot_color: STATUS_ERROR,
            };
        }

        match &self.sync_run_status {
            SyncRunStatus::Running { .. } => TitleSyncStatus {
                label: "Sync".to_string(),
                dot_color: STATUS_SYNCING,
            },
            SyncRunStatus::Error { .. } => TitleSyncStatus {
                label: "Sync".to_string(),
                dot_color: STATUS_ERROR,
            },
            _ if pending > 0 => TitleSyncStatus {
                label: "Sync".to_string(),
                dot_color: STATUS_PENDING,
            },
            _ => TitleSyncStatus {
                label: "Sync".to_string(),
                dot_color: STATUS_OK,
            },
        }
    }

    /// Largest of locally-pending CRDT edits and the count reported by the last
    /// sync run, so the indicator never under-reports unsynced work.
    pub(crate) fn sync_pending_count(&self) -> usize {
        let local_pending = self.state.pending_crdt_edits().len();
        let pending_from_run = match &self.sync_run_status {
            SyncRunStatus::Running { pending }
            | SyncRunStatus::Synced { pending }
            | SyncRunStatus::Error { pending, .. } => *pending,
            SyncRunStatus::Idle => 0,
        };
        local_pending.max(pending_from_run)
    }
}
