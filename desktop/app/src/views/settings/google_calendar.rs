use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context};
use knotq_l10n::{t as tr, t_count as tr_count, t_with as tr_with};
use knotq_model::{CalendarProvider, SchemeId, SchemeSource};

use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme as UiTheme};

use super::components::{
    settings_message, settings_subheading, title_detail_column, SettingsRowStyle,
};
use super::labels::google_calendar_last_synced_label;

pub(super) struct GoogleCalendarSettingsRow {
    scheme_id: SchemeId,
    title: String,
    detail: String,
    connected: bool,
}

impl KnotQApp {
    pub(super) fn google_calendar_account_rows(
        &mut self,
        t: UiTheme,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        let calendar_rows = self.google_calendar_settings_rows();

        if self.settings.google_accounts.is_empty() {
            rows.push(settings_message(
                if calendar_rows.is_empty() {
                    tr("settings.google_calendar.no_accounts").to_string()
                } else {
                    tr("settings.google_calendar.accounts_offline_notice").to_string()
                },
                false,
                t,
            ));
        } else {
            rows.push(settings_subheading(
                tr("settings.google_calendar.accounts_heading"),
                t,
            ));
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
                        let detail =
                            tr_count("settings.google_calendar.calendar_count", count as i64);
                        google_account_row(idx, account_id, title, detail, t, cx)
                    }),
            );
        }

        rows.push(settings_subheading(
            tr("settings.google_calendar.calendars_heading"),
            t,
        ));
        if calendar_rows.is_empty() {
            rows.push(settings_message(
                tr("settings.google_calendar.no_calendars").to_string(),
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
                    tr("settings.google_calendar.status_on")
                } else {
                    tr("settings.google_calendar.status_not_connected")
                };
                let synced = source
                    .last_synced_at
                    .map(google_calendar_last_synced_label)
                    .unwrap_or_else(|| tr("settings.google_calendar.not_synced_yet").to_string());
                Some(GoogleCalendarSettingsRow {
                    scheme_id: scheme.id,
                    title: self.scheme_display_name(scheme),
                    detail: tr_with(
                        "settings.google_calendar.row_detail",
                        &[
                            ("status", status),
                            ("account_label", &account_label),
                            ("synced", &synced),
                        ],
                    ),
                    connected,
                })
            })
            .collect::<Vec<_>>();

        rows.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.detail.cmp(&b.detail)));
        rows
    }
}

fn google_calendar_row(
    idx: usize,
    row: GoogleCalendarSettingsRow,
    t: UiTheme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let scheme_id = row.scheme_id;
    let connected = row.connected;
    let button_id = if connected {
        "google-calendar-unlink"
    } else {
        "google-calendar-link"
    };
    let button_label = if connected {
        tr("settings.google_calendar.unlink_button")
    } else {
        tr("settings.google_calendar.link_button")
    };

    div()
        .id(("google-calendar-setting", idx))
        .px(px(8.0))
        .py(px(4.0))
        .min_h(px(36.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .bottom_divider(t)
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(7.0))
                .child(google_calendar_status_dot(row.connected))
                .child(title_detail_column(row.title, row.detail, t)),
        )
        .child(
            div()
                .id((button_id, idx))
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
                    if connected {
                        this.request_delete_scheme(scheme_id, cx);
                    } else {
                        this.start_google_calendar_scheme_reconnect(scheme_id, cx);
                    }
                }))
                .child(button_label),
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
        .bottom_divider(t)
        .child(title_detail_column(title, detail, t))
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
                .child(tr("settings.google_calendar.unlink_button")),
        )
        .into_any_element()
}
