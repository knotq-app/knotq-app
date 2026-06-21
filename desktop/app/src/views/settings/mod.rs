mod components;
mod google_calendar;
mod labels;
mod sync_panel;

use gpui::prelude::*;
use gpui::{div, px, Context};
use gpui_component::scroll::ScrollableElement as _;
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
    notification_offset_label, theme_mode_label, time_format_label,
};

impl KnotQApp {
    pub fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme();
        let theme_rows = vec![settings_dropdown_group(
            "theme-setting",
            "Theme",
            SettingsDropdown::Theme,
            theme_mode_label(self.theme_mode),
            vec![
                ("Dark", ThemeMode::Dark),
                ("Light", ThemeMode::Light),
                ("System", ThemeMode::System),
            ],
            self.theme_mode,
            self.settings_dropdown == Some(SettingsDropdown::Theme),
            t,
            cx,
            |this, mode, cx| this.set_theme_mode(mode, cx),
        )];

        let mut calendar_rows = vec![settings_dropdown_group(
            "calendar-view-setting",
            "View",
            SettingsDropdown::CalendarView,
            calendar_view_label(self.calendar_view),
            vec![
                ("Week", CalendarViewMode::Week),
                ("Month", CalendarViewMode::Month),
            ],
            self.calendar_view,
            self.settings_dropdown == Some(SettingsDropdown::CalendarView),
            t,
            cx,
            |this, mode, cx| this.set_calendar_view(mode, cx),
        )];
        calendar_rows.push(settings_dropdown_group(
            "calendar-range-setting",
            "Range",
            SettingsDropdown::CalendarRange,
            calendar_range_label(self.calendar_week_range),
            vec![
                ("Rolling week", CalendarWeekRange::NextSevenDays),
                ("Calendar week", CalendarWeekRange::CalendarWeek),
            ],
            self.calendar_week_range,
            self.settings_dropdown == Some(SettingsDropdown::CalendarRange),
            t,
            cx,
            |this, range, cx| this.set_calendar_week_range(range, cx),
        ));

        let time_rows = vec![settings_dropdown_group(
            "time-format-setting",
            "Clock",
            SettingsDropdown::TimeFormat,
            time_format_label(self.time_format),
            vec![
                ("12-hour", TimeFormat::TwelveHour),
                ("24-hour", TimeFormat::TwentyFourHour),
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
            "Events",
            SettingsDropdown::EventNotification,
            notification_offset_label(self.notification_defaults.event_offset_secs),
            vec![
                ("At start", 0),
                ("5 min", 5 * 60),
                ("10 min", DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS),
                ("15 min", 15 * 60),
                ("30 min", 30 * 60),
                ("1 hr", 60 * 60),
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
            "Assignments",
            SettingsDropdown::AssignmentNotification,
            assignment_notification_offset_label(self.notification_defaults.assignment_offset_secs),
            vec![
                ("At due", 0),
                ("1 hr", 60 * 60),
                ("2 hr", 2 * 60 * 60),
                ("6 hr", 6 * 60 * 60),
                ("1 day", 24 * 60 * 60),
                ("2 days", 2 * 24 * 60 * 60),
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
}
