use chrono::{DateTime, Local, Utc};
use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement};
use gpui_component::scroll::ScrollableElement as _;
use knotq_model::{
    CalendarProvider, SchemeId, SchemeSource, DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS,
};
use knotq_storage_json::{CalendarViewMode, CalendarWeekRange, ThemeMode, TimeFormat};

use crate::app::auto_update::AutoUpdateUiStatus;
use crate::app::KnotQApp;
use crate::theme_gpui::{all_themes, token_hsla, token_rgba, Theme as UiTheme};
use crate::views::sync_account::{sync_cta_bg, sync_cta_hover_bg};

struct GoogleCalendarSettingsRow {
    scheme_id: SchemeId,
    title: String,
    detail: String,
    connected: bool,
}

impl KnotQApp {
    pub fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme();
        let themes = all_themes();

        let theme_rows = vec![settings_chip_group(
            "Theme",
            [
                ("Dark", ThemeMode::Dark, themes[0]),
                ("Light", ThemeMode::Light, themes[1]),
                ("System", ThemeMode::System, self.theme()),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, (label, mode, theme))| {
                let is_active = self.theme_mode == mode;
                choice_chip(
                    ("theme", idx),
                    label,
                    is_active,
                    theme_swatch(theme, t),
                    t,
                    cx,
                    move |this, cx| this.set_theme_mode(mode, cx),
                )
            })
            .collect(),
            t,
        )];

        let mut calendar_rows = Vec::new();
        calendar_rows.push(settings_chip_group(
            "View",
            [
                ("Week", CalendarViewMode::Week),
                ("Month", CalendarViewMode::Month),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, (label, mode))| {
                let is_active = self.calendar_view == mode;
                choice_chip(
                    ("calendar-setting", idx),
                    label,
                    is_active,
                    active_marker(is_active, t),
                    t,
                    cx,
                    move |this, cx| this.set_calendar_view(mode, cx),
                )
            })
            .collect(),
            t,
        ));
        calendar_rows.push(settings_chip_group(
            "Range",
            [
                ("Rolling week", CalendarWeekRange::NextSevenDays),
                ("Calendar week", CalendarWeekRange::CalendarWeek),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, (label, range))| {
                let is_active = self.calendar_week_range == range;
                choice_chip(
                    ("calendar-range-setting", idx),
                    label,
                    is_active,
                    active_marker(is_active, t),
                    t,
                    cx,
                    move |this, cx| this.set_calendar_week_range(range, cx),
                )
            })
            .collect(),
            t,
        ));

        let time_rows = vec![settings_chip_group(
            "Clock",
            [
                ("12-hour", TimeFormat::TwelveHour),
                ("24-hour", TimeFormat::TwentyFourHour),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, (label, format))| {
                let is_active = self.time_format == format;
                choice_chip(
                    ("time-format-setting", idx),
                    label,
                    is_active,
                    active_marker(is_active, t),
                    t,
                    cx,
                    move |this, cx| this.set_time_format(format, cx),
                )
            })
            .collect(),
            t,
        )];

        let mut notification_rows: Vec<gpui::AnyElement> = Vec::new();
        notification_rows.push(settings_chip_group(
            "Events",
            [
                ("At start", 0),
                ("5 min", 5 * 60),
                ("10 min", DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS),
                ("15 min", 15 * 60),
                ("30 min", 30 * 60),
                ("1 hr", 60 * 60),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, (label, offset_secs))| {
                let is_active = self.notification_defaults.event_offset_secs == offset_secs;
                let mut defaults = self.notification_defaults;
                defaults.event_offset_secs = offset_secs;
                choice_chip(
                    ("event-notification-setting", idx),
                    label,
                    is_active,
                    active_marker(is_active, t),
                    t,
                    cx,
                    move |this, cx| this.set_notification_defaults(defaults, cx),
                )
            })
            .collect(),
            t,
        ));
        notification_rows.push(settings_chip_group(
            "Assignments",
            [
                ("At due", 0),
                ("1 hr", 60 * 60),
                ("2 hr", 2 * 60 * 60),
                ("6 hr", 6 * 60 * 60),
                ("1 day", 24 * 60 * 60),
                ("2 days", 2 * 24 * 60 * 60),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, (label, offset_secs))| {
                let is_active = self.notification_defaults.assignment_offset_secs == offset_secs;
                let mut defaults = self.notification_defaults;
                defaults.assignment_offset_secs = offset_secs;
                choice_chip(
                    ("assignment-notification-setting", idx),
                    label,
                    is_active,
                    active_marker(is_active, t),
                    t,
                    cx,
                    move |this, cx| this.set_notification_defaults(defaults, cx),
                )
            })
            .collect(),
            t,
        ));
        let update_rows = self.auto_update_rows(t, cx);
        let sync_panel = self.settings_sync_panel(t, cx);
        let google_rows = self.google_calendar_account_rows(t, cx);

        div()
            .flex_1()
            .h_full()
            .bg(token_hsla(t.bg_app))
            .overflow_y_scrollbar()
            .child(
                div().w_full().flex().justify_center().child(
                    div()
                        .w_full()
                        .max_w(px(620.0))
                        .px(px(12.0))
                        .pt(px(8.0))
                        .pb(px(80.0))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(settings_header(t))
                        .child(sync_panel)
                        .child(settings_section("Appearance", theme_rows, t))
                        .child(settings_section("Calendar", calendar_rows, t))
                        .child(settings_section("Google Calendar", google_rows, t))
                        .child(settings_section("Time", time_rows, t))
                        .child(settings_section("Notifications", notification_rows, t))
                        .child(settings_section("Updates", update_rows, t)),
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

    fn settings_sync_panel(&mut self, t: UiTheme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let account = self.settings.sync_account.as_ref();
        let signed_in = account.is_some();
        let sync_enabled = account.is_some_and(|account| account.supports_sync);
        let (badge, default_detail, badge_bg, badge_fg) = settings_sync_panel_state(
            signed_in,
            sync_enabled,
            account.map(|account| account.email.as_str()),
            t,
        );

        div()
            .w_full()
            .rounded(px(6.0))
            .border_1()
            .border_color(token_rgba(settings_sync_panel_border(t)))
            .bg(token_rgba(settings_sync_panel_bg(t)))
            .shadow_md()
            .p(px(9.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(settings_sync_glyph(t))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(token_hsla(t.text_primary))
                                            .child("KnotQ Sync"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .line_height(px(13.0))
                                            .text_color(token_hsla(t.text_soft))
                                            .child(default_detail),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(99.0))
                            .bg(token_rgba(badge_bg))
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(token_hsla(badge_fg))
                            .child(badge),
                    ),
            )
            .child(self.sync_account_management_section(t, cx))
            .into_any_element()
    }

    fn google_calendar_account_rows(
        &mut self,
        t: UiTheme,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        let calendar_rows = self.google_calendar_settings_rows();

        if self.settings.google_accounts.is_empty() {
            rows.push(settings_message(
                if calendar_rows.is_empty() {
                    "No Google accounts connected locally.".to_string()
                } else {
                    "No local Google account credentials. Calendars below will stay offline until you reconnect.".to_string()
                },
                false,
                t,
            ));
        } else {
            rows.push(settings_subheading("Accounts", t));
            rows.extend(
                self.settings
                    .google_accounts
                    .clone()
                    .into_iter()
                    .enumerate()
                    .map(|(idx, account)| {
                        let account_id = account.account_id.clone();
                        let title = account
                            .email
                            .clone()
                            .filter(|email| !email.trim().is_empty())
                            .unwrap_or_else(|| account.account_id.clone());
                        let count = self.google_calendar_scheme_count_for_account(&account);
                        let detail = match count {
                            0 => "0 calendars".to_string(),
                            1 => "1 calendar".to_string(),
                            count => format!("{count} calendars"),
                        };
                        google_account_row(idx, account_id, title, detail, t, cx)
                    }),
            );
        }

        rows.push(settings_subheading("Calendars", t));
        if calendar_rows.is_empty() {
            rows.push(settings_message(
                "No Google calendars imported.".to_string(),
                false,
                t,
            ));
        } else {
            rows.extend(
                calendar_rows
                    .into_iter()
                    .enumerate()
                    .map(|(idx, row)| google_calendar_row(idx, row, t, cx)),
            );
        }

        rows
    }

    fn google_calendar_settings_rows(&self) -> Vec<GoogleCalendarSettingsRow> {
        let mut rows = self
            .workspace
            .schemes
            .values()
            .filter_map(|scheme| {
                if self.workspace.is_scheme_deleted(scheme.id) {
                    return None;
                }
                let SchemeSource::ImportedCalendar(source) = &scheme.source else {
                    return None;
                };
                if source.provider != CalendarProvider::Google {
                    return None;
                }

                let connected = self.google_calendar_has_local_credentials(scheme);
                let account_label = self
                    .imported_calendar_account_label(scheme)
                    .unwrap_or_else(|| source.account_id.clone());
                let status = if connected {
                    "On"
                } else {
                    "Not connected on this device"
                };
                let synced = source
                    .last_synced_at
                    .map(google_calendar_last_synced_label)
                    .unwrap_or_else(|| "Not synced yet".to_string());
                Some(GoogleCalendarSettingsRow {
                    scheme_id: scheme.id,
                    title: self.scheme_display_name(scheme),
                    detail: format!("{status} - {account_label} - {synced}"),
                    connected,
                })
            })
            .collect::<Vec<_>>();

        rows.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.detail.cmp(&b.detail)));
        rows
    }
}

fn settings_sync_panel_state(
    signed_in: bool,
    sync_enabled: bool,
    email: Option<&str>,
    t: UiTheme,
) -> (&'static str, String, u32, u32) {
    if sync_enabled {
        return (
            "Enabled",
            email.unwrap_or("Sync on").to_string(),
            if t.is_dark { 0x30d15826 } else { 0x1f8f4d18 },
            if t.is_dark { 0x9af0b6ff } else { 0x176b38ff },
        );
    }

    if signed_in {
        return (
            "Upgrade",
            email.unwrap_or("Sync off").to_string(),
            if t.is_dark { 0xf59e0b28 } else { 0xd977061a },
            if t.is_dark { 0xf8d38dff } else { 0x9a4b00ff },
        );
    }

    (
        "Available",
        "Sign in".to_string(),
        if t.is_dark { 0x3b82f628 } else { 0x2f67cf18 },
        if t.is_dark { 0x9bc2ffff } else { 0x235ebeff },
    )
}

fn settings_sync_panel_bg(t: UiTheme) -> u32 {
    if t.is_dark {
        0x3b82f616
    } else {
        0xeaf2ffff
    }
}

fn settings_sync_panel_border(t: UiTheme) -> u32 {
    if t.is_dark {
        0x7aa0ff44
    } else {
        0x2f67cf38
    }
}

/// The brand mark: the actual KnotQ app icon, so the card is recognizably ours
/// rather than a generic glyph.
fn settings_sync_glyph(_t: UiTheme) -> gpui::AnyElement {
    div()
        .w(px(30.0))
        .h(px(30.0))
        .flex_shrink_0()
        .rounded(px(6.0))
        .overflow_hidden()
        .child(
            gpui::img("app-icon/128x128.png")
                .w(px(30.0))
                .h(px(30.0))
                .object_fit(gpui::ObjectFit::Cover),
        )
        .into_any_element()
}

fn settings_header(t: UiTheme) -> gpui::AnyElement {
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

fn settings_section(
    title: &'static str,
    rows: Vec<gpui::AnyElement>,
    t: UiTheme,
) -> gpui::AnyElement {
    div()
        .w_full()
        .border_t_1()
        .border_color(token_rgba(t.divider_soft))
        .pt(px(5.0))
        .child(
            div().px(px(2.0)).pb(px(4.0)).child(
                div()
                    .text_size(px(11.0))
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

fn settings_chip_group(
    label: &'static str,
    chips: Vec<gpui::AnyElement>,
    t: UiTheme,
) -> gpui::AnyElement {
    div()
        .px(px(8.0))
        .py(px(5.0))
        .min_h(px(34.0))
        .flex()
        .items_start()
        .gap(px(8.0))
        .border_b_1()
        .border_color(token_rgba(t.divider_tiny))
        .child(
            div()
                .w(px(86.0))
                .flex_shrink_0()
                .pt(px(4.0))
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_dim))
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .children(chips),
        )
        .into_any_element()
}

fn choice_chip<F>(
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
        .min_h(px(26.0))
        .px(px(7.0))
        .py(px(3.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(token_rgba(if is_active {
            settings_selection_accent(t)
        } else {
            t.border_main
        }))
        .bg(token_rgba(if is_active {
            settings_selection_bg(t)
        } else {
            t.button_bg
        }))
        .cursor_pointer()
        .hover({
            let c = if is_active {
                settings_selection_bg(t)
            } else {
                t.button_hover
            };
            move |h| h.bg(token_rgba(c))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            on_click(this, cx);
        }))
        .child(marker)
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(token_hsla(t.text_primary))
                .whitespace_nowrap()
                .child(label),
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
        .py(px(3.0))
        .min_h(px(28.0))
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
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(label),
                ),
        )
        .into_any_element()
}

fn update_status_row(
    status: AutoUpdateUiStatus,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    match status {
        AutoUpdateUiStatus::Idle => settings_action_row(
            "auto-update-check",
            "Current version".to_string(),
            format!("KnotQ {}", env!("CARGO_PKG_VERSION")),
            "Check",
            t,
            cx,
            false,
            |this, cx| this.check_for_updates(cx),
        ),
        AutoUpdateUiStatus::Checking => {
            settings_message("Checking for updates...".to_string(), false, t)
        }
        AutoUpdateUiStatus::Available { update, .. } => settings_action_row(
            "auto-update-download",
            format!("KnotQ {} is available", update.version),
            update.asset.name,
            "Update",
            t,
            cx,
            true,
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
                "auto-update-install",
                format!("KnotQ {} is ready", update.version),
                update.asset_name,
                button,
                t,
                cx,
                true,
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
            false,
            |this, cx| this.check_for_updates(cx),
        ),
        AutoUpdateUiStatus::Errored {
            message, update, ..
        } => {
            let has_retry = update.is_some();
            settings_action_row(
                "auto-update-check",
                if has_retry {
                    "Update failed".to_string()
                } else {
                    "Update check failed".to_string()
                },
                message,
                if has_retry { "Retry" } else { "Check" },
                t,
                cx,
                has_retry,
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

fn google_calendar_row(
    idx: usize,
    row: GoogleCalendarSettingsRow,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let scheme_id = row.scheme_id;

    div()
        .id(("google-calendar-setting", idx))
        .px(px(8.0))
        .py(px(4.0))
        .min_h(px(36.0))
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
                .items_center()
                .gap(px(7.0))
                .child(google_calendar_status_dot(row.connected))
                .child(
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
                                .child(row.title),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .line_height(px(13.0))
                                .text_color(token_hsla(t.text_soft))
                                .child(row.detail),
                        ),
                ),
        )
        .child(
            div()
                .id(("google-calendar-unlink", idx))
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
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.request_delete_scheme(scheme_id, cx);
                }))
                .child("Unlink"),
        )
        .into_any_element()
}

fn google_calendar_status_dot(connected: bool) -> gpui::AnyElement {
    div()
        .w(px(7.0))
        .h(px(7.0))
        .flex_shrink_0()
        .rounded(px(99.0))
        .bg(token_rgba(if connected { 0x30d158ff } else { 0xf59e0bff }))
        .into_any_element()
}

fn google_account_row(
    idx: usize,
    account_id: String,
    title: String,
    detail: String,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let confirm_label = title.clone();

    div()
        .id(("google-account-setting", idx))
        .px(px(8.0))
        .py(px(4.0))
        .min_h(px(34.0))
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
                ),
        )
        .child(
            div()
                .id(("google-account-unlink", idx))
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
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.request_unlink_google_account(
                        account_id.clone(),
                        confirm_label.clone(),
                        cx,
                    );
                }))
                .child("Unlink"),
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
    primary: bool,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut KnotQApp, &mut Context<KnotQApp>) + 'static,
{
    div()
        .id(id)
        .px(px(8.0))
        .py(px(4.0))
        .min_h(px(34.0))
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

fn checked_time_label(checked_at: DateTime<Utc>) -> String {
    checked_at.with_timezone(&Local).format("%H:%M").to_string()
}

fn google_calendar_last_synced_label(value: DateTime<Utc>) -> String {
    format!(
        "Synced {}",
        value.with_timezone(&Local).format("%b %-d %H:%M")
    )
}

fn settings_subheading(label: &'static str, t: UiTheme) -> gpui::AnyElement {
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

fn settings_message(message: String, is_error: bool, t: UiTheme) -> gpui::AnyElement {
    div()
        .px(px(8.0))
        .py(px(4.0))
        .min_h(px(28.0))
        .border_b_1()
        .border_color(token_rgba(t.divider_tiny))
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
