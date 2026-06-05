use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement, MouseButton, Window, WindowControlArea};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, Sizable};
use knotq_commands::Command;
use knotq_model::SchemeId;
use knotq_storage_json::CalendarViewMode;

use crate::app::auto_update::AutoUpdateUiStatus;
use crate::app::{daily_queue_marker_color, KnotQApp, SyncAuthStatus, SyncRunStatus, View};
use crate::theme_gpui::{
    palette_hsla, scheme_color, token_hsla, token_rgba, Theme, FONT_SIZE_HEADLINE,
};

const TITLE_CONTENT_W: f32 = 430.0;
const LINUX_TITLE_CONTENT_W: f32 = 340.0;
const LINUX_WINDOW_CONTROLS_W: f32 = 132.0;
const TITLE_MARKER_SIZE: f32 = 18.0;
const TITLE_TEXT_W: f32 = 190.0;
const LINUX_TITLE_TEXT_W: f32 = 150.0;
const COLOR_SWATCH_ORDER: &[u8] = &[0, 1, 5, 2, 3, 4];

// Semantic sync status colors (the theme has no status palette of its own).
pub(crate) const STATUS_OK: u32 = 0x22c55eff;
pub(crate) const STATUS_SYNCING: u32 = 0x3b82f6ff;
pub(crate) const STATUS_PENDING: u32 = 0xf59e0bff;
pub(crate) const STATUS_ERROR: u32 = 0xef4444ff;

struct TitleSyncStatus {
    label: String,
    dot_color: u32,
}

impl KnotQApp {
    pub(crate) fn render_title_bar(
        &mut self,
        window: &mut Window,
        view: View,
        title: String,
        scheme: Option<(SchemeId, String, u8)>,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let linux_client_decorations = Self::uses_linux_client_decorations();
        let title_content_w = if linux_client_decorations {
            LINUX_TITLE_CONTENT_W
        } else {
            TITLE_CONTENT_W
        };
        let title_text_w = if linux_client_decorations {
            LINUX_TITLE_TEXT_W
        } else {
            TITLE_TEXT_W
        };

        let base = div()
            .relative()
            .flex()
            .items_center()
            .h(px(38.0))
            .pl(px(80.0))
            .pr(px(16.0))
            .bg(token_hsla(t.bg_cal_hdr))
            .border_b_1()
            .border_color(token_rgba(t.divider))
            .when(linux_client_decorations, |s| {
                s.pr(px(16.0 + LINUX_WINDOW_CONTROLS_W))
            });

        let active_scheme = scheme.as_ref().filter(|_| view == View::Scheme);
        let marker_color = if let Some((_, _, color_index)) = active_scheme {
            palette_hsla(scheme_color(*color_index, t.is_dark), 1.0)
        } else if view == View::Union {
            token_hsla(t.text_highlight)
        } else if view == View::DailyQueue {
            token_hsla(daily_queue_marker_color(t.is_dark))
        } else {
            token_hsla(t.text_dim)
        };

        let mut color_swatches: Vec<gpui::AnyElement> = Vec::new();
        if let Some((scheme_id, _, color_index)) = active_scheme {
            for (i, color_ix) in COLOR_SWATCH_ORDER.iter().copied().enumerate() {
                let is_active = *color_index == color_ix;
                let dot = palette_hsla(scheme_color(color_ix, t.is_dark), 1.0);
                let active_border = t.caret_color;
                color_swatches.push(
                    div()
                        .id(("title-color-sw", i))
                        .w(px(18.0))
                        .h(px(18.0))
                        .rounded(px(3.0))
                        .bg(dot)
                        .border_1()
                        .border_color(token_rgba(if is_active {
                            active_border
                        } else {
                            0x00000000
                        }))
                        .cursor_pointer()
                        .on_click({
                            let scheme_id = *scheme_id;
                            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                                this.apply(
                                    Command::SetSchemeColor {
                                        id: scheme_id,
                                        color_index: color_ix,
                                    },
                                    cx,
                                );
                            })
                        })
                        .into_any_element(),
                );
            }
        }

        let mut calendar_mode_controls: Vec<gpui::AnyElement> = Vec::new();
        if view == View::Union {
            for (i, (label, mode)) in [
                ("Week", CalendarViewMode::Week),
                ("Month", CalendarViewMode::Month),
            ]
            .into_iter()
            .enumerate()
            {
                let is_active = self.calendar_view == mode;
                calendar_mode_controls.push(
                    div()
                        .id(("title-calendar-mode", i))
                        .h_full()
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .font_weight(if is_active {
                            gpui::FontWeight::SEMIBOLD
                        } else {
                            gpui::FontWeight::NORMAL
                        })
                        .text_color(token_hsla(if is_active {
                            t.text_primary
                        } else {
                            t.text_muted
                        }))
                        .when(is_active, {
                            let c = t.row_selected;
                            move |s| s.bg(token_rgba(c))
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.set_calendar_view(mode, cx);
                        }))
                        .child(label)
                        .into_any_element(),
                );
            }
        }

        let search_control = self.render_title_bar_search(window, t, cx);
        let sync_control = self.render_title_bar_sync_control(t, cx);
        let update_control = self.render_title_bar_update_control(t, cx);

        base.child(
            div()
                .id("title-drag-region")
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right_0()
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(WindowControlArea::Drag)
                .when(linux_client_decorations, |s| {
                    s.on_mouse_down(MouseButton::Left, |_, window, cx| {
                        cx.stop_propagation();
                        window.start_window_move();
                    })
                    .on_click(|event, window, cx| {
                        cx.stop_propagation();
                        if event.click_count() == 2 {
                            window.zoom_window();
                        } else if event.is_right_click() {
                            window.show_window_menu(event.position());
                        }
                    })
                })
                .child(
                    div()
                        .w(px(title_content_w))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .w(px(TITLE_MARKER_SIZE))
                                .h(px(TITLE_MARKER_SIZE))
                                .rounded(px(3.0))
                                .bg(marker_color),
                        )
                        .child(
                            div()
                                .w(px(title_text_w))
                                .min_w_0()
                                .truncate()
                                .text_size(px(FONT_SIZE_HEADLINE))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(token_hsla(t.text_primary))
                                .child(title),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .gap(px(8.0))
                .when_some(sync_control, |s, sync_control| s.child(sync_control))
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .window_control_area(WindowControlArea::Drag),
                ),
        )
        .child(div().w(px(title_content_w)).flex_shrink_0().h_full())
        .child(
            div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(color_swatches),
                )
                .when(!calendar_mode_controls.is_empty(), move |s| {
                    s.child(
                        div()
                            .h(px(26.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(token_rgba(t.border_soft))
                            .bg(token_rgba(t.button_bg))
                            .p(px(2.0))
                            .flex()
                            .items_center()
                            .children(calendar_mode_controls),
                    )
                })
                .when_some(update_control, |s, update_control| s.child(update_control))
                .child(search_control)
                .child(
                    div()
                        .id("title-settings")
                        .h(px(26.0))
                        .w(px(28.0))
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
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.open_settings();
                            this.focus_app_root(window);
                            cx.notify();
                        }))
                        .child(
                            Icon::new(IconName::Settings)
                                .xsmall()
                                .text_color(token_hsla(t.text_dim)),
                        ),
                ),
        )
        .children(self.render_linux_window_controls(window, t, cx))
        .into_any_element()
    }

    fn render_title_bar_update_control(
        &self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let AutoUpdateUiStatus::Ready { update } = &self.auto_update_status else {
            return None;
        };

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

        Some(
            div()
                .id("title-auto-update")
                .h(px(26.0))
                .px(px(9.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .bg(token_rgba(if t.is_dark { 0x3f7cff24 } else { 0x2f67cf18 }))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.install_ready_update(cx);
                }))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .child(
                    Icon::new(IconName::Redo2)
                        .xsmall()
                        .text_color(token_hsla(t.text_highlight)),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(label),
                )
                .into_any_element(),
        )
    }

    fn render_title_bar_sync_control(
        &self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.settings.sync_account.is_none()
            && !matches!(self.sync_auth_status, SyncAuthStatus::InProgress)
        {
            return None;
        }
        let status = self.title_sync_status(t);
        let popover_open = self.sync_status_popover.is_some();

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
                .on_click(cx.listener(|this, event: &ClickEvent, _window, cx| {
                    this.toggle_sync_status_popover(event.position(), cx);
                }))
                .child(
                    div()
                        .w(px(7.0))
                        .h(px(7.0))
                        .rounded(px(4.0))
                        .bg(token_rgba(status.dot_color)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(token_hsla(t.text_dim))
                        .child(status.label),
                )
                .into_any_element(),
        )
    }

    fn title_sync_status(&self, t: Theme) -> TitleSyncStatus {
        let account = self.settings.sync_account.as_ref();
        let pending = self.sync_pending_count();

        if matches!(self.sync_auth_status, SyncAuthStatus::InProgress) {
            return TitleSyncStatus {
                label: "Signing in".to_string(),
                dot_color: STATUS_SYNCING,
            };
        }

        if account.is_none() {
            return TitleSyncStatus {
                label: "Sign in".to_string(),
                dot_color: t.text_muted,
            };
        }

        if account.is_some_and(|account| !account.supports_sync) {
            return TitleSyncStatus {
                label: "Sync off".to_string(),
                dot_color: STATUS_ERROR,
            };
        }

        match &self.sync_run_status {
            SyncRunStatus::Running { .. } => TitleSyncStatus {
                label: "Syncing".to_string(),
                dot_color: STATUS_SYNCING,
            },
            SyncRunStatus::Error { .. } => TitleSyncStatus {
                label: "Sync error".to_string(),
                dot_color: STATUS_ERROR,
            },
            _ if pending > 0 => TitleSyncStatus {
                label: "Pending".to_string(),
                dot_color: STATUS_PENDING,
            },
            _ => TitleSyncStatus {
                label: "Synced".to_string(),
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

    fn uses_linux_client_decorations() -> bool {
        cfg!(target_os = "linux")
    }

    fn render_linux_window_controls(
        &self,
        window: &mut Window,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !Self::uses_linux_client_decorations() {
            return None;
        }

        let controls = window.window_controls();
        Some(
            div()
                .id("linux-window-controls")
                .absolute()
                .top_0()
                .right_0()
                .h_full()
                .w(px(LINUX_WINDOW_CONTROLS_W))
                .flex()
                .items_center()
                .justify_end()
                .bg(token_rgba(t.bg_cal_hdr))
                .child(Self::linux_window_control_button(
                    "linux-window-minimize",
                    Self::linux_minimize_glyph(t),
                    false,
                    controls.minimize,
                    |_: &ClickEvent, window, _cx| window.minimize_window(),
                    t,
                ))
                .child(Self::linux_window_control_button(
                    "linux-window-maximize",
                    Self::linux_maximize_glyph(t),
                    false,
                    controls.maximize,
                    |_: &ClickEvent, window, _cx| window.zoom_window(),
                    t,
                ))
                .child(Self::linux_window_control_button(
                    "linux-window-close",
                    Self::linux_close_glyph(),
                    true,
                    true,
                    cx.listener(|this, _: &ClickEvent, window, _cx| {
                        this.flush_for_shutdown("linux title bar close");
                        window.remove_window();
                    }),
                    t,
                ))
                .into_any_element(),
        )
    }

    fn linux_window_control_button(
        id: &'static str,
        glyph: gpui::AnyElement,
        is_close: bool,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
        t: Theme,
    ) -> gpui::AnyElement {
        let hover_bg = if is_close {
            if t.is_dark {
                0xff5a537d
            } else {
                0xd20f3988
            }
        } else {
            t.button_hover
        };

        div()
            .id(id)
            .w(px(44.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .text_size(px(12.0))
            .text_color(token_hsla(if is_close {
                t.text_primary
            } else {
                t.text_dim
            }))
            .when(enabled, |s| {
                s.cursor_pointer()
                    .hover(move |h| h.bg(token_rgba(hover_bg)))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(move |event: &ClickEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        on_click(event, window, cx);
                    })
            })
            .when(!enabled, |s| s.opacity(0.35))
            .child(glyph)
            .into_any_element()
    }

    fn linux_minimize_glyph(t: Theme) -> gpui::AnyElement {
        div()
            .w(px(10.0))
            .h(px(1.5))
            .rounded(px(1.0))
            .bg(token_rgba(t.text_dim))
            .into_any_element()
    }

    fn linux_maximize_glyph(t: Theme) -> gpui::AnyElement {
        div()
            .w(px(9.0))
            .h(px(9.0))
            .rounded(px(1.5))
            .border_1()
            .border_color(token_rgba(t.text_dim))
            .into_any_element()
    }

    fn linux_close_glyph() -> gpui::AnyElement {
        div().child("x").into_any_element()
    }

    fn render_title_bar_search(
        &mut self,
        window: &mut Window,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui_component::input::Input;
        use gpui_component::Sizable as _;

        if self.search_open {
            let input = self.ensure_search_input(window, cx);
            div()
                .id("title-search")
                .h(px(26.0))
                .w(px(236.0))
                .pl(px(7.0))
                .pr(px(8.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .bg(token_rgba(t.button_bg))
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    let input = this.ensure_search_input(window, cx);
                    input.update(cx, |input, cx| input.focus(window, cx));
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .child(
                            Input::new(&input)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .xsmall()
                                .w_full()
                                .h_full(),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(token_hsla(t.text_muted))
                        .child("⌘F"),
                )
                .into_any_element()
        } else {
            div()
                .id("title-search")
                .h(px(26.0))
                .w(px(108.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .bg(token_rgba(t.button_bg))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_search(window, cx);
                }))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(token_hsla(t.text_dim))
                        .child("search"),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(token_hsla(t.text_muted))
                        .child("⌘F"),
                )
                .into_any_element()
        }
    }
}
