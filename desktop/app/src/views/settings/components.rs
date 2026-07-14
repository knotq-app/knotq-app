use gpui::prelude::*;
use gpui::{deferred, div, px, ClickEvent, Context, IntoElement};
use gpui_component::{Icon, IconName, Sizable};

use crate::app::auto_update::AutoUpdateUiStatus;
use crate::app::{KnotQApp, SettingsDropdown};
use crate::theme_gpui::{token_hsla, token_rgba, Theme as UiTheme};
use crate::views::sync_account::{sync_cta_bg, sync_cta_hover_bg};

use super::labels::checked_time_label;

/// The hairline that separates stacked settings rows. Shared by every row
/// builder so the divider treatment stays identical across the panel.
pub(super) trait SettingsRowStyle: Sized {
    fn bottom_divider(self, t: UiTheme) -> Self;
}

impl<T: Styled> SettingsRowStyle for T {
    fn bottom_divider(self, t: UiTheme) -> Self {
        self.border_b_1().border_color(token_rgba(t.divider_tiny))
    }
}

/// The stacked title + muted detail column shared by the calendar/account/action
/// rows. Same type sizes and colors in every caller.
pub(super) fn title_detail_column(title: String, detail: String, t: UiTheme) -> gpui::AnyElement {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_primary))
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.0))
                .line_height(px(13.0))
                .text_color(token_hsla(t.text_soft))
                .child(detail),
        )
        .into_any_element()
}

pub(super) fn settings_header(t: UiTheme) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .pb(px(0.0))
        .child(
            div()
                .text_size(px(17.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_primary))
                .child("Settings"),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(token_hsla(t.text_soft))
                .child(format!("KnotQ {}", env!("CARGO_PKG_VERSION"))),
        )
        .into_any_element()
}

pub(super) fn settings_section(
    title: &'static str,
    rows: Vec<gpui::AnyElement>,
    t: UiTheme,
) -> gpui::AnyElement {
    div()
        .w_full()
        .pt(px(6.0))
        .child(
            div().px(px(2.0)).pb(px(4.0)).child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_primary))
                    .child(title),
            ),
        )
        .child(div().flex().flex_col().children(rows))
        .into_any_element()
}

/// Bundled plain-data parameters for [`settings_dropdown_group`]. `T` is the
/// value type of each dropdown option (generic per-caller, e.g. `ThemeMode`).
pub(super) struct SettingsDropdownGroupArgs<T> {
    pub id: &'static str,
    pub label: &'static str,
    pub dropdown: SettingsDropdown,
    pub selected_label: &'static str,
    pub options: Vec<(&'static str, T)>,
    pub current: T,
    pub is_open: bool,
    pub t: UiTheme,
}

pub(super) fn settings_dropdown_group<T, F>(
    args: SettingsDropdownGroupArgs<T>,
    cx: &mut Context<KnotQApp>,
    on_select: F,
) -> gpui::AnyElement
where
    T: Copy + PartialEq + 'static,
    F: Fn(&mut KnotQApp, T, &mut Context<KnotQApp>) + Copy + 'static,
{
    let SettingsDropdownGroupArgs {
        id,
        label,
        dropdown,
        selected_label,
        options,
        current,
        is_open,
        t,
    } = args;
    let option_rows = options
        .into_iter()
        .enumerate()
        .map(|(idx, (option_label, value))| {
            let selected = value == current;
            div()
                .id((id, idx + 1))
                .w_full()
                .min_h(px(26.0))
                .px(px(7.0))
                .py(px(3.0))
                .flex()
                .items_center()
                .gap(px(7.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .when(selected, {
                    let c = settings_selection_bg(t);
                    move |s| s.bg(token_rgba(c))
                })
                .when(!selected, {
                    let c = t.row_hover;
                    move |s| s.hover(move |h| h.bg(token_rgba(c)))
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    on_select(this, value, cx);
                    this.settings_dropdown = None;
                    cx.notify();
                }))
                .child(active_marker(selected, t))
                .child(
                    div()
                        .whitespace_nowrap()
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(option_label),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

    div()
        .px(px(8.0))
        .py(px(5.0))
        .min_h(px(34.0))
        .flex()
        .items_start()
        .gap(px(8.0))
        .bottom_divider(t)
        .child(
            div()
                .w(px(86.0))
                .flex_shrink_0()
                .pt(px(5.0))
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_dim))
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .relative()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .id((id, 0_usize))
                        .h(px(28.0))
                        .max_w(px(240.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(token_rgba(if is_open {
                            settings_selection_accent(t)
                        } else {
                            t.border_main
                        }))
                        .bg(token_rgba(t.button_bg))
                        .cursor_pointer()
                        .hover({
                            let c = t.button_hover;
                            move |h| h.bg(token_rgba(c))
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.settings_dropdown = if this.settings_dropdown == Some(dropdown) {
                                None
                            } else {
                                Some(dropdown)
                            };
                            cx.notify();
                        }))
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_size(px(11.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(token_hsla(t.text_primary))
                                .child(selected_label),
                        )
                        .child(
                            Icon::new(if is_open {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .with_size(px(13.0))
                            .text_color(token_hsla(t.text_soft)),
                        ),
                )
                .when(is_open, |s| {
                    s.child(deferred(
                        div()
                            .absolute()
                            .top(px(32.0))
                            .left_0()
                            .w_full()
                            .max_w(px(240.0))
                            .p(px(3.0))
                            .rounded(px(5.0))
                            .border_1()
                            .border_color(token_rgba(t.border_main))
                            .bg(token_rgba(t.bg_modal))
                            .shadow_md()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children(option_rows),
                    ))
                }),
        )
        .into_any_element()
}

pub(super) fn choice_row<F>(
    id: (&'static str, usize),
    label: &'static str,
    is_active: bool,
    marker: gpui::AnyElement,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut KnotQApp, &mut Context<KnotQApp>) + 'static,
{
    div()
        .id(id)
        .px(px(8.0))
        .py(px(3.0))
        .min_h(px(28.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .bottom_divider(t)
        .cursor_pointer()
        .when(is_active, {
            let c = settings_selection_bg(t);
            move |s| s.bg(token_rgba(c))
        })
        .when(!is_active, {
            let c = t.row_hover;
            move |s| s.hover(move |h| h.bg(token_rgba(c)))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            on_click(this, cx);
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(7.0))
                .min_w_0()
                .child(marker)
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(label),
                ),
        )
        .into_any_element()
}

pub(super) fn update_status_row(
    status: AutoUpdateUiStatus,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    match status {
        AutoUpdateUiStatus::Idle => settings_action_row(
            SettingsActionRowArgs {
                id: "auto-update-check",
                title: "Current version".to_string(),
                detail: format!("KnotQ {}", env!("CARGO_PKG_VERSION")),
                button_label: "Check",
                primary: false,
            },
            t,
            cx,
            |this, cx| this.check_for_updates(cx),
        ),
        AutoUpdateUiStatus::Checking => {
            settings_message("Checking for updates...".to_string(), false, t)
        }
        AutoUpdateUiStatus::Available { update, .. } => settings_action_row(
            SettingsActionRowArgs {
                id: "auto-update-download",
                title: format!("KnotQ {} is available", update.version),
                detail: update.asset.name,
                button_label: "Update",
                primary: true,
            },
            t,
            cx,
            |this, cx| this.download_available_update(cx),
        ),
        AutoUpdateUiStatus::Downloading { version } => {
            settings_message(format!("Updating KnotQ {version}..."), false, t)
        }
        AutoUpdateUiStatus::Ready { update } => {
            let button = match update.install_strategy {
                knotq_auto_update::InstallStrategy::InstalledOnRestart => "Restart",
                knotq_auto_update::InstallStrategy::RunInstallerAndQuit => "Install",
            };
            settings_action_row(
                SettingsActionRowArgs {
                    id: "auto-update-install",
                    title: format!("KnotQ {} is ready", update.version),
                    detail: update.asset_name,
                    button_label: button,
                    primary: true,
                },
                t,
                cx,
                |this, cx| this.install_ready_update(cx),
            )
        }
        AutoUpdateUiStatus::UpToDate {
            version,
            checked_at,
        } => settings_action_row(
            SettingsActionRowArgs {
                id: "auto-update-check",
                title: "KnotQ is up to date".to_string(),
                detail: format!(
                    "Latest: {version} - checked {}",
                    checked_time_label(checked_at)
                ),
                button_label: "Check",
                primary: false,
            },
            t,
            cx,
            |this, cx| this.check_for_updates(cx),
        ),
        AutoUpdateUiStatus::Errored {
            message, update, ..
        } => {
            let has_retry = update.is_some();
            settings_action_row(
                SettingsActionRowArgs {
                    id: "auto-update-check",
                    title: if has_retry {
                        "Update failed".to_string()
                    } else {
                        "Update check failed".to_string()
                    },
                    detail: message,
                    button_label: if has_retry { "Retry" } else { "Check" },
                    primary: has_retry,
                },
                t,
                cx,
                move |this, cx| {
                    if has_retry {
                        this.download_available_update(cx);
                    } else {
                        this.check_for_updates(cx);
                    }
                },
            )
        }
    }
}

/// Bundled plain-data parameters for [`settings_action_row`].
pub(super) struct SettingsActionRowArgs {
    pub id: &'static str,
    pub title: String,
    pub detail: String,
    pub button_label: &'static str,
    pub primary: bool,
}

pub(super) fn settings_action_row<F>(
    args: SettingsActionRowArgs,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut KnotQApp, &mut Context<KnotQApp>) + 'static,
{
    let SettingsActionRowArgs {
        id,
        title,
        detail,
        button_label,
        primary,
    } = args;
    div()
        .id(id)
        .px(px(8.0))
        .py(px(4.0))
        .min_h(px(34.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .bottom_divider(t)
        .child(title_detail_column(title, detail, t))
        .child(
            div()
                .id((id, 0_usize))
                .flex_shrink_0()
                .px(px(7.0))
                .py(px(3.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(token_rgba(if primary {
                    sync_cta_bg()
                } else {
                    t.border_main
                }))
                .bg(token_rgba(if primary {
                    sync_cta_bg()
                } else {
                    t.button_bg
                }))
                .hover(move |h| {
                    h.bg(token_rgba(if primary {
                        sync_cta_hover_bg()
                    } else {
                        t.button_hover
                    }))
                })
                .cursor_pointer()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(if primary {
                    0xffffffff
                } else {
                    t.text_primary
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    on_click(this, cx);
                }))
                .child(button_label),
        )
        .into_any_element()
}

pub(super) fn settings_subheading(label: &'static str, t: UiTheme) -> gpui::AnyElement {
    div()
        .px(px(8.0))
        .pt(px(5.0))
        .pb(px(2.0))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(token_hsla(t.text_dim))
        .child(label)
        .into_any_element()
}

pub(super) fn settings_message(message: String, is_error: bool, t: UiTheme) -> gpui::AnyElement {
    div()
        .px(px(8.0))
        .py(px(4.0))
        .min_h(px(28.0))
        .bottom_divider(t)
        .bg(token_rgba(if is_error { 0xde5b2524 } else { 0x00000000 }))
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(14.0))
                .text_color(token_hsla(if is_error {
                    t.text_today
                } else {
                    t.text_soft
                }))
                .child(message),
        )
        .into_any_element()
}

pub(super) fn active_marker(is_active: bool, t: UiTheme) -> gpui::AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .border_1()
        .border_color(token_rgba(if is_active {
            settings_selection_accent(t)
        } else {
            t.border_main
        }))
        .bg(token_rgba(t.button_bg))
        .when(is_active, |s| {
            s.child(
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded(px(1.0))
                    .bg(token_rgba(settings_selection_accent(t))),
            )
        })
        .into_any_element()
}

pub(super) fn settings_selection_accent(t: UiTheme) -> u32 {
    if t.is_dark {
        0x7aa0ffff
    } else {
        0x2f67cfff
    }
}

pub(super) fn settings_selection_bg(t: UiTheme) -> u32 {
    if t.is_dark {
        0x3f7cff24
    } else {
        0x2f67cf18
    }
}
