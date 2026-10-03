use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, Sizable};
use knotq_auto_update::InstallStrategy;
use knotq_l10n::{t as tr, t_with as tr_with};

use crate::app::auto_update::AutoUpdateUiStatus;
use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme};
use crate::views::{sync_cta_bg, sync_cta_hover_bg};

#[derive(Clone, Copy)]
enum TitleUpdateAction {
    Download,
    Install,
}

/// What the title bar offers for an update status.
///
/// Kept separate from rendering because one rule here is the whole point of
/// staging updates early: an update being fetched in the background is
/// [`Hidden`](TitleUpdateState::Hidden). The button appears when taking the
/// update is a restart, not when it is a download the user would wait through.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TitleUpdateState {
    /// Nothing in the title bar.
    Hidden,
    /// Something the user started is running; shown, but not clickable.
    Working { version: String },
    /// An update that has not been fetched yet, so taking it means downloading.
    Download { version: String, retry: bool },
    /// A staged update, ready to be installed.
    Install {
        version: String,
        strategy: InstallStrategy,
    },
}

pub(crate) fn title_bar_update_state(status: &AutoUpdateUiStatus) -> TitleUpdateState {
    match status {
        AutoUpdateUiStatus::Available { update, .. } => TitleUpdateState::Download {
            version: update.version.to_string(),
            retry: false,
        },
        AutoUpdateUiStatus::Errored {
            update: Some(update),
            ..
        } => TitleUpdateState::Download {
            version: update.version.to_string(),
            retry: true,
        },
        // A download nobody asked for stays silent: it is on its way to becoming
        // an Install, and a pill that sits at "Updating" for minutes says nothing
        // the user can act on.
        AutoUpdateUiStatus::Downloading {
            user_initiated: false,
            ..
        } => TitleUpdateState::Hidden,
        AutoUpdateUiStatus::Downloading { version, .. }
        | AutoUpdateUiStatus::Installing { version } => TitleUpdateState::Working {
            version: version.clone(),
        },
        AutoUpdateUiStatus::Ready { update } => TitleUpdateState::Install {
            version: update.version.to_string(),
            strategy: update.install_strategy.clone(),
        },
        AutoUpdateUiStatus::Idle
        | AutoUpdateUiStatus::Checking
        | AutoUpdateUiStatus::UpToDate { .. }
        | AutoUpdateUiStatus::Errored { update: None, .. } => TitleUpdateState::Hidden,
    }
}

impl KnotQApp {
    pub(super) fn render_title_bar_update_control(
        &self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let (label, tooltip, action) = match title_bar_update_state(&self.auto_update_status) {
            TitleUpdateState::Hidden => return None,
            TitleUpdateState::Working { version } => (
                tr("update.action.updating"),
                tr_with("update.tooltip.updating", &[("version", version.as_str())]),
                None,
            ),
            TitleUpdateState::Download { version, retry } => (
                tr("update.action.update"),
                tr_with(
                    if retry {
                        "update.tooltip.retry_download"
                    } else {
                        "update.tooltip.update_and_restart"
                    },
                    &[("version", version.as_str())],
                ),
                Some(TitleUpdateAction::Download),
            ),
            TitleUpdateState::Install { version, strategy } => {
                let (label, tooltip) = match strategy {
                    InstallStrategy::InstalledOnRestart => (
                        tr("update.action.restart_to_update"),
                        "update.tooltip.restart_to_finish",
                    ),
                    InstallStrategy::RunInstallerAndQuit => (
                        tr("update.action.install_update"),
                        "update.tooltip.run_installer",
                    ),
                };
                (
                    label,
                    tr_with(tooltip, &[("version", version.as_str())]),
                    Some(TitleUpdateAction::Install),
                )
            }
        };
        let actionable = action.is_some();
        let button_bg = if actionable {
            sync_cta_bg()
        } else {
            t.button_bg
        };
        let button_border = if actionable {
            sync_cta_bg()
        } else {
            t.border_soft
        };
        let button_text = if actionable { 0xffffffff } else { t.text_dim };

        Some(
            div()
                .id("title-auto-update")
                .h(px(26.0))
                .min_w(px(92.0))
                .px(px(10.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(token_rgba(button_border))
                .bg(token_rgba(button_bg))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .when(actionable, |button| {
                    button
                        .cursor_pointer()
                        .hover(|s| s.bg(token_rgba(sync_cta_hover_bg())))
                })
                .when_some(action, |button, action| match action {
                    TitleUpdateAction::Download => {
                        button.on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.download_available_update(cx);
                        }))
                    }
                    TitleUpdateAction::Install => {
                        button.on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.install_ready_update(cx);
                        }))
                    }
                })
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .child(
                    Icon::new(IconName::Redo2)
                        .xsmall()
                        .text_color(token_hsla(button_text)),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(button_text))
                        .child(label),
                )
                .into_any_element(),
        )
    }
}
