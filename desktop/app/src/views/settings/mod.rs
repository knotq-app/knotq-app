mod components;
mod google_calendar;
mod labels;
mod sync_panel;

use gpui::prelude::*;
use gpui::{div, px, Context};
use gpui_component::scroll::ScrollableElement as _;
use knotq_l10n::t as tr;
use knotq_model::DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS;
use knotq_storage_json::{CalendarViewMode, CalendarWeekRange, ThemeMode, TimeFormat};

use crate::app::{KnotQApp, SettingsDropdown};
use crate::theme_gpui::{token_hsla, Theme as UiTheme};

use components::{
    active_marker, choice_row, settings_dropdown_group, settings_header, settings_section,
    update_status_row,
};
use labels::{
    assignment_notification_offset_label, calendar_range_label, calendar_view_label,
    current_language_value, language_label, language_options, notification_offset_label,
    theme_mode_label, time_format_label,
};

impl KnotQApp {
    pub fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme();
        let mut theme_rows = vec![settings_dropdown_group(
            "theme-setting",
            tr("settings.appearance.theme_label"),
            SettingsDropdown::Theme,
            theme_mode_label(self.theme_mode),
            vec![
                (tr("settings.appearance.theme_dark"), ThemeMode::Dark),
                (tr("settings.appearance.theme_light"), ThemeMode::Light),
                (tr("settings.appearance.theme_system"), ThemeMode::System),
            ],
            self.theme_mode,
            self.settings_dropdown == Some(SettingsDropdown::Theme),
            t,
            cx,
            |this, mode, cx| this.set_theme_mode(mode, cx),
        )];
        let current_language = current_language_value(self.settings.language.as_deref());
        theme_rows.push(settings_dropdown_group(
            "language-setting",
            tr("settings.language.label"),
            SettingsDropdown::Language,
            language_label(current_language),
            language_options(),
            current_language,
            self.settings_dropdown == Some(SettingsDropdown::Language),
            t,
            cx,
            |this, code, cx| this.set_language(code.map(|c| c.to_string()), cx),
        ));

        let mut calendar_rows = vec![settings_dropdown_group(
            "calendar-view-setting",
            tr("settings.calendar.view_label"),
            SettingsDropdown::CalendarView,
            calendar_view_label(self.calendar_view),
            vec![
                (tr("settings.calendar.view_week"), CalendarViewMode::Week),
                (tr("settings.calendar.view_month"), CalendarViewMode::Month),
            ],
            self.calendar_view,
            self.settings_dropdown == Some(SettingsDropdown::CalendarView),
            t,
            cx,
            |this, mode, cx| this.set_calendar_view(mode, cx),
        )];
        calendar_rows.push(settings_dropdown_group(
            "calendar-range-setting",
            tr("settings.calendar.range_label"),
            SettingsDropdown::CalendarRange,
            calendar_range_label(self.calendar_week_range),
            vec![
                (
                    tr("settings.calendar.range_rolling_week"),
                    CalendarWeekRange::NextSevenDays,
                ),
                (
                    tr("settings.calendar.range_calendar_week"),
                    CalendarWeekRange::CalendarWeek,
                ),
            ],
            self.calendar_week_range,
            self.settings_dropdown == Some(SettingsDropdown::CalendarRange),
            t,
            cx,
            |this, range, cx| this.set_calendar_week_range(range, cx),
        ));

        let time_rows = vec![settings_dropdown_group(
            "time-format-setting",
            tr("settings.time.clock_label"),
            SettingsDropdown::TimeFormat,
            time_format_label(self.time_format),
            vec![
                (tr("settings.time.clock_12h"), TimeFormat::TwelveHour),
                (tr("settings.time.clock_24h"), TimeFormat::TwentyFourHour),
            ],
            self.time_format,
            self.settings_dropdown == Some(SettingsDropdown::TimeFormat),
            t,
            cx,
            |this, format, cx| this.set_time_format(format, cx),
        )];

        let mut notification_rows: Vec<gpui::AnyElement> = Vec::new();
        notification_rows.push(settings_dropdown_group(
            "event-notification-setting",
            tr("settings.notifications.events_label"),
            SettingsDropdown::EventNotification,
            notification_offset_label(self.notification_defaults.event_offset_secs),
            vec![
                (tr("settings.notifications.offset_at_start"), 0),
                (tr("settings.notifications.offset_5_min"), 5 * 60),
                (
                    tr("settings.notifications.offset_10_min"),
                    DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS,
                ),
                (tr("settings.notifications.offset_15_min"), 15 * 60),
                (tr("settings.notifications.offset_30_min"), 30 * 60),
                (tr("settings.notifications.offset_1_hr"), 60 * 60),
            ],
            self.notification_defaults.event_offset_secs,
            self.settings_dropdown == Some(SettingsDropdown::EventNotification),
            t,
            cx,
            |this, offset_secs, cx| {
                let mut defaults = this.notification_defaults;
                defaults.event_offset_secs = offset_secs;
                this.set_notification_defaults(defaults, cx);
            },
        ));
        notification_rows.push(settings_dropdown_group(
            "assignment-notification-setting",
            tr("settings.notifications.assignments_label"),
            SettingsDropdown::AssignmentNotification,
            assignment_notification_offset_label(self.notification_defaults.assignment_offset_secs),
            vec![
                (tr("settings.notifications.offset_at_due"), 0),
                (tr("settings.notifications.offset_1_hr"), 60 * 60),
                (tr("settings.notifications.offset_2_hr"), 2 * 60 * 60),
                (tr("settings.notifications.offset_6_hr"), 6 * 60 * 60),
                (tr("settings.notifications.offset_1_day"), 24 * 60 * 60),
                (
                    tr("settings.notifications.offset_2_days"),
                    2 * 24 * 60 * 60,
                ),
            ],
            self.notification_defaults.assignment_offset_secs,
            self.settings_dropdown == Some(SettingsDropdown::AssignmentNotification),
            t,
            cx,
            |this, offset_secs, cx| {
                let mut defaults = this.notification_defaults;
                defaults.assignment_offset_secs = offset_secs;
                this.set_notification_defaults(defaults, cx);
            },
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
                        .child(settings_section(
                            tr("settings.appearance.section"),
                            theme_rows,
                            t,
                        ))
                        .child(settings_section(
                            tr("settings.calendar.section"),
                            calendar_rows,
                            t,
                        ))
                        .child(settings_section(
                            tr("settings.google_calendar.section"),
                            google_rows,
                            t,
                        ))
                        .child(settings_section(tr("settings.time.section"), time_rows, t))
                        .child(settings_section(
                            tr("settings.notifications.section"),
                            notification_rows,
                            t,
                        ))
                        .child(settings_section(
                            tr("settings.updates.section"),
                            update_rows,
                            t,
                        )),
                ),
            )
            .into_any_element()
    }

    fn auto_update_rows(&mut self, t: UiTheme, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let auto_update_enabled = self.settings.auto_update;
        let mut rows = vec![choice_row(
            ("auto-update-setting", 0),
            tr("settings.updates.auto_check"),
            auto_update_enabled,
            active_marker(auto_update_enabled, t),
            t,
            cx,
            move |this, cx| this.set_auto_update_enabled(!auto_update_enabled, cx),
        )];

        rows.push(update_status_row(self.auto_update_status.clone(), t, cx));
        rows
    }
}
