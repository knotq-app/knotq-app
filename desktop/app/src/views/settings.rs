use chrono::{DateTime, Duration, Local, Utc};
use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement};
use gpui_component::scroll::ScrollableElement as _;
use knotq_storage_json::{
    list_workspace_snapshots, workspace_dir, CalendarViewMode, CalendarWeekRange,
    NotificationDefaults, ThemeMode, TimeFormat, WorkspaceSnapshot,
};

use crate::app::auto_update::AutoUpdateUiStatus;
use crate::app::KnotQApp;
use crate::theme_gpui::{all_themes, token_hsla, token_rgba, Theme as UiTheme};

impl KnotQApp {
    pub fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme();
        let themes = all_themes();

        let theme_rows = [
            ("Dark", ThemeMode::Dark, themes[0]),
            ("Light", ThemeMode::Light, themes[1]),
            ("System", ThemeMode::System, self.theme()),
        ]
        .into_iter()
        .enumerate()
        .map(|(idx, (label, mode, theme))| {
            let is_active = self.theme_mode == mode;
            choice_row(
                ("theme", idx),
                label,
                is_active,
                theme_swatch(theme, t),
                t,
                cx,
                move |this, cx| this.set_theme_mode(mode, cx),
            )
        })
        .collect::<Vec<_>>();

        let mut calendar_rows = Vec::new();
        calendar_rows.push(settings_subheading("View", t));
        calendar_rows.extend(
            [
                ("Week", CalendarViewMode::Week),
                ("Month", CalendarViewMode::Month),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, (label, mode))| {
                let is_active = self.calendar_view == mode;
                choice_row(
                    ("calendar-setting", idx),
                    label,
                    is_active,
                    active_marker(is_active, t),
                    t,
                    cx,
                    move |this, cx| this.set_calendar_view(mode, cx),
                )
            })
            .collect::<Vec<_>>(),
        );
        calendar_rows.push(settings_subheading("Week Range", t));
        calendar_rows.extend(
            [
                ("Yesterday + 6 days", CalendarWeekRange::NextSevenDays),
                ("Calendar week", CalendarWeekRange::CalendarWeek),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, (label, range))| {
                let is_active = self.calendar_week_range == range;
                choice_row(
                    ("calendar-range-setting", idx),
                    label,
                    is_active,
                    active_marker(is_active, t),
                    t,
                    cx,
                    move |this, cx| this.set_calendar_week_range(range, cx),
                )
            })
            .collect::<Vec<_>>(),
        );

        let time_rows = [
            ("12-hour", TimeFormat::TwelveHour),
            ("24-hour", TimeFormat::TwentyFourHour),
        ]
        .into_iter()
        .enumerate()
        .map(|(idx, (label, format))| {
            let is_active = self.time_format == format;
            choice_row(
                ("time-format-setting", idx),
                label,
                is_active,
                active_marker(is_active, t),
                t,
                cx,
                move |this, cx| this.set_time_format(format, cx),
            )
        })
        .collect::<Vec<_>>();

        let mut notification_rows: Vec<gpui::AnyElement> = Vec::new();
        notification_rows.push(settings_subheading("Events", t));
        for (idx, (label, offset_secs)) in [
            ("At start", 0),
            ("5 minutes before", 5 * 60),
            ("10 minutes before", 10 * 60),
            ("15 minutes before", 15 * 60),
            ("30 minutes before", 30 * 60),
            ("1 hour before", 60 * 60),
        ]
        .into_iter()
        .enumerate()
        {
            let is_active = self.notification_defaults.event_offset_secs == offset_secs;
            let mut defaults = self.notification_defaults;
            defaults.event_offset_secs = offset_secs;
            notification_rows.push(notification_choice_row(
                ("event-notification-setting", idx),
                label,
                is_active,
                defaults,
                t,
                cx,
            ));
        }
        notification_rows.push(settings_subheading("Assignments", t));
        for (idx, (label, offset_secs)) in [
            ("At due time", 0),
            ("1 hour before", 60 * 60),
            ("2 hours before", 2 * 60 * 60),
            ("6 hours before", 6 * 60 * 60),
            ("1 day before", 24 * 60 * 60),
            ("2 days before", 2 * 24 * 60 * 60),
        ]
        .into_iter()
        .enumerate()
        {
            let is_active = self.notification_defaults.assignment_offset_secs == offset_secs;
            let mut defaults = self.notification_defaults;
            defaults.assignment_offset_secs = offset_secs;
            notification_rows.push(notification_choice_row(
                ("assignment-notification-setting", idx),
                label,
                is_active,
                defaults,
                t,
                cx,
            ));
        }
        let history_rows = self.version_history_rows(t, cx);
        let update_rows = self.auto_update_rows(t, cx);
        let sync_rows = self.sync_account_rows(t, cx);

        div()
            .flex_1()
            .h_full()
            .bg(token_hsla(t.bg_app))
            .overflow_y_scrollbar()
            .child(
                div().w_full().flex().justify_center().child(
                    div()
                        .w_full()
                        .max_w(px(560.0))
                        .px(px(16.0))
                        .pt(px(10.0))
                        .pb(px(16.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(settings_header(t))
                        .child(settings_section("Appearance", theme_rows, t))
                        .child(settings_section("Calendar", calendar_rows, t))
                        .child(settings_section("Time", time_rows, t))
                        .child(settings_section("Notifications", notification_rows, t))
                        .child(settings_section("Sync", sync_rows, t))
                        .child(settings_section("Updates", update_rows, t))
                        .child(settings_section("Version History", history_rows, t)),
                ),
            )
            .into_any_element()
    }

    fn auto_update_rows(&mut self, t: UiTheme, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let auto_update_enabled = self.settings.auto_update;
        let mut rows = vec![choice_row(
            ("auto-update-setting", 0),
            "Automatically check for updates",
            auto_update_enabled,
            active_marker(auto_update_enabled, t),
            t,
            cx,
            move |this, cx| this.set_auto_update_enabled(!auto_update_enabled, cx),
        )];

        rows.push(update_status_row(self.auto_update_status.clone(), t, cx));
        rows
    }

    fn sync_account_rows(&mut self, t: UiTheme, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let (title, detail, action) = match self.settings.sync_account.as_ref() {
            Some(account) => (
                format!("Signed in as {}", account.email),
                if account.supports_sync {
                    account.api_base.clone()
                } else {
                    format!("{} - sync not allowed", account.api_base)
                },
                "Manage",
            ),
            None => (
                "Not signed in".to_string(),
                "Connect to the local Cloudflare sync Worker.".to_string(),
                "Sign in",
            ),
        };

        vec![sync_account_row(title, detail, action, t, cx)]
    }

    fn version_history_rows(
        &mut self,
        t: UiTheme,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        if let Some(err) = &self.workspace_history_error {
            rows.push(settings_message(err.clone(), true, t));
        }
        match list_workspace_snapshots(&workspace_dir()) {
            Ok(snapshots) if snapshots.is_empty() => {
                rows.push(settings_message(
                    "No saved versions yet.".to_string(),
                    false,
                    t,
                ));
            }
            Ok(snapshots) => {
                rows.extend(
                    snapshots
                        .into_iter()
                        .take(24)
                        .enumerate()
                        .map(|(idx, snapshot)| history_snapshot_row(idx, snapshot, t, cx)),
                );
            }
            Err(err) => rows.push(settings_message(
                format!("Version history unavailable: {err:#}"),
                true,
                t,
            )),
        }
        rows
    }
}

fn settings_header(t: UiTheme) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .pb(px(1.0))
        .child(
            div()
                .text_size(px(18.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_primary))
                .child("Settings"),
        )
        .child(
            div()
                .text_size(px(11.0))
                .line_height(px(14.0))
                .text_color(token_hsla(t.text_soft))
                .child(format!("KnotQ {}", env!("CARGO_PKG_VERSION"))),
        )
        .into_any_element()
}

fn settings_section(
    title: &'static str,
    rows: Vec<gpui::AnyElement>,
    t: UiTheme,
) -> gpui::AnyElement {
    div()
        .w_full()
        .border_t_1()
        .border_color(token_rgba(t.divider_soft))
        .pt(px(7.0))
        .child(
            div().px(px(2.0)).pb(px(5.0)).child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_soft))
                    .child(title),
            ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(token_rgba(t.divider_tiny))
                .children(rows),
        )
        .into_any_element()
}

fn choice_row<F>(
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
        .py(px(4.0))
        .min_h(px(30.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .border_b_1()
        .border_color(token_rgba(t.divider_tiny))
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
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(label),
                ),
        )
        .into_any_element()
}

fn notification_choice_row(
    id: (&'static str, usize),
    label: &'static str,
    is_active: bool,
    defaults: NotificationDefaults,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    choice_row(
        id,
        label,
        is_active,
        active_marker(is_active, t),
        t,
        cx,
        move |this, cx| this.set_notification_defaults(defaults, cx),
    )
}

fn history_snapshot_row(
    idx: usize,
    snapshot: WorkspaceSnapshot,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let snapshot_id = snapshot.id.clone();
    let short_id = snapshot.id.chars().take(8).collect::<String>();
    let detail = snapshot_detail(&snapshot, short_id);
    div()
        .id(("version-history", idx))
        .px(px(8.0))
        .py(px(5.0))
        .min_h(px(38.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .border_b_1()
        .border_color(token_rgba(t.divider_tiny))
        .cursor_pointer()
        .hover({
            let c = t.row_hover;
            move |h| h.bg(token_rgba(c))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.restore_workspace_to_snapshot(snapshot_id.clone(), cx);
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(7.0))
                .min_w_0()
                .child(
                    div()
                        .w(px(2.0))
                        .h(px(24.0))
                        .flex_shrink_0()
                        .rounded(px(1.0))
                        .bg(token_rgba(t.border_main)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(token_hsla(t.text_primary))
                                .child(snapshot.label),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .line_height(px(14.0))
                                .text_color(token_hsla(t.text_soft))
                                .child(detail),
                        ),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(token_rgba(t.border_main))
                .bg(token_rgba(t.button_bg))
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_primary))
                .child("Restore"),
        )
        .into_any_element()
}

fn update_status_row(
    status: AutoUpdateUiStatus,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    match status {
        AutoUpdateUiStatus::Disabled { reason } => settings_message(reason, false, t),
        AutoUpdateUiStatus::Idle => settings_action_row(
            "auto-update-check",
            "Current version".to_string(),
            format!("KnotQ {}", env!("CARGO_PKG_VERSION")),
            "Check",
            t,
            cx,
            |this, cx| this.check_for_updates(cx),
        ),
        AutoUpdateUiStatus::Checking => {
            settings_message("Checking for updates...".to_string(), false, t)
        }
        AutoUpdateUiStatus::Downloading { version } => {
            settings_message(format!("Downloading KnotQ {version}..."), false, t)
        }
        AutoUpdateUiStatus::Ready { update } => {
            let button = match update.install_strategy {
                knotq_auto_update::InstallStrategy::InstalledOnRestart => "Restart",
                knotq_auto_update::InstallStrategy::RunInstallerAndQuit => "Install",
            };
            settings_action_row(
                "auto-update-install",
                format!("KnotQ {} is ready", update.version),
                update.asset_name,
                button,
                t,
                cx,
                |this, cx| this.install_ready_update(cx),
            )
        }
        AutoUpdateUiStatus::UpToDate {
            version,
            checked_at,
        } => settings_action_row(
            "auto-update-check",
            "KnotQ is up to date".to_string(),
            format!(
                "Latest: {version} - checked {}",
                checked_time_label(checked_at)
            ),
            "Check",
            t,
            cx,
            |this, cx| this.check_for_updates(cx),
        ),
        AutoUpdateUiStatus::Errored { message, .. } => settings_action_row(
            "auto-update-check",
            "Update check failed".to_string(),
            message,
            "Check",
            t,
            cx,
            |this, cx| this.check_for_updates(cx),
        ),
    }
}

fn sync_account_row(
    title: String,
    detail: String,
    button_label: &'static str,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id("sync-account-setting")
        .px(px(8.0))
        .py(px(5.0))
        .min_h(px(38.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .border_b_1()
        .border_color(token_rgba(t.divider_tiny))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(14.0))
                        .text_color(token_hsla(t.text_soft))
                        .child(detail),
                ),
        )
        .child(
            div()
                .id(("sync-account-setting", 0_usize))
                .flex_shrink_0()
                .px(px(7.0))
                .py(px(3.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(token_rgba(t.border_main))
                .bg(token_rgba(t.button_bg))
                .hover({
                    let c = t.button_hover;
                    move |h| h.bg(token_rgba(c))
                })
                .cursor_pointer()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_primary))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_sync_sign_in(window, cx);
                }))
                .child(button_label),
        )
        .into_any_element()
}

fn settings_action_row<F>(
    id: &'static str,
    title: String,
    detail: String,
    button_label: &'static str,
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
        .py(px(5.0))
        .min_h(px(38.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .border_b_1()
        .border_color(token_rgba(t.divider_tiny))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(14.0))
                        .text_color(token_hsla(t.text_soft))
                        .child(detail),
                ),
        )
        .child(
            div()
                .id((id, 0_usize))
                .flex_shrink_0()
                .px(px(7.0))
                .py(px(3.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(token_rgba(t.border_main))
                .bg(token_rgba(t.button_bg))
                .hover({
                    let c = t.button_hover;
                    move |h| h.bg(token_rgba(c))
                })
                .cursor_pointer()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_primary))
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    on_click(this, cx);
                }))
                .child(button_label),
        )
        .into_any_element()
}

fn checked_time_label(checked_at: DateTime<Utc>) -> String {
    checked_at.with_timezone(&Local).format("%H:%M").to_string()
}

fn snapshot_detail(snapshot: &WorkspaceSnapshot, short_id: String) -> String {
    let age = Utc::now().signed_duration_since(snapshot.timestamp);
    if age >= Duration::zero() && age <= Duration::hours(3) {
        let minutes = age.num_minutes();
        return format!("{minutes} min ago - {short_id}");
    }
    short_id
}

fn settings_subheading(label: &'static str, t: UiTheme) -> gpui::AnyElement {
    div()
        .px(px(8.0))
        .pt(px(7.0))
        .pb(px(3.0))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(token_hsla(t.text_dim))
        .child(label)
        .into_any_element()
}

fn settings_message(message: String, is_error: bool, t: UiTheme) -> gpui::AnyElement {
    div()
        .px(px(8.0))
        .py(px(5.0))
        .min_h(px(30.0))
        .border_b_1()
        .border_color(token_rgba(t.divider_tiny))
        .bg(token_rgba(if is_error { 0xde5b2524 } else { 0x00000000 }))
        .child(
            div()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(token_hsla(if is_error {
                    t.text_today
                } else {
                    t.text_soft
                }))
                .child(message),
        )
        .into_any_element()
}

fn theme_swatch(theme: UiTheme, t: UiTheme) -> gpui::AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(token_rgba(t.border_main))
        .bg(token_rgba(theme.bg_app))
        .into_any_element()
}

fn active_marker(is_active: bool, t: UiTheme) -> gpui::AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
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

fn settings_selection_accent(t: UiTheme) -> u32 {
    if t.is_dark {
        0x7aa0ffff
    } else {
        0x2f67cfff
    }
}

fn settings_selection_bg(t: UiTheme) -> u32 {
    if t.is_dark {
        0x3f7cff24
    } else {
        0x2f67cf18
    }
}
