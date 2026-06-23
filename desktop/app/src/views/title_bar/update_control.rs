use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, Sizable};

use crate::app::auto_update::AutoUpdateUiStatus;
use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme};
use crate::views::sync_account::{sync_cta_bg, sync_cta_hover_bg};

#[derive(Clone, Copy)]
enum TitleUpdateAction {
    Download,
    Install,
}

impl KnotQApp {
    pub(super) fn render_title_bar_update_control(
        &self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let (label, tooltip, action) = match &self.auto_update_status {
            AutoUpdateUiStatus::Available { update, .. } => (
                "Update",
                format!("Update and restart KnotQ to {}.", update.version),
                Some(TitleUpdateAction::Download),
            ),
            AutoUpdateUiStatus::Downloading { version } => {
                ("Updating", format!("Updating KnotQ {version}..."), None)
            }
            AutoUpdateUiStatus::Ready { update } => {
                let label = match update.install_strategy {
                    knotq_auto_update::InstallStrategy::InstalledOnRestart => "Restart to update",
                    knotq_auto_update::InstallStrategy::RunInstallerAndQuit => "Install update",
                };
                let tooltip = match update.install_strategy {
                    knotq_auto_update::InstallStrategy::InstalledOnRestart => {
                        format!("Restart KnotQ to finish updating to {}.", update.version)
                    }
                    knotq_auto_update::InstallStrategy::RunInstallerAndQuit => {
                        format!("Run the KnotQ {} installer.", update.version)
                    }
                };
                (label, tooltip, Some(TitleUpdateAction::Install))
            }
            AutoUpdateUiStatus::Errored {
                update: Some(update),
                ..
            } => (
                "Update",
                format!("Retry downloading KnotQ {}.", update.version),
                Some(TitleUpdateAction::Download),
            ),
            _ => return None,
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
