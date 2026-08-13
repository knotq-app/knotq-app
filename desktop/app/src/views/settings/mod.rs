mod components;
mod google_calendar;
mod labels;
#[cfg(feature = "accounts")]
mod sync_panel;

use gpui::prelude::*;
use gpui::{div, px, Context};
use gpui_component::scroll::ScrollableElement as _;
use knotq_l10n::t as tr;
use knotq_model::DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS;
use knotq_storage_json::{CalendarViewMode, CalendarWeekRange, ThemeMode, TimeFormat};

use crate::app::{KnotQApp, SettingsDropdown};
use crate::theme_gpui::{token_hsla, token_rgba, Theme as UiTheme};

use components::{
    active_marker, choice_row, settings_dropdown_group, settings_header, settings_navigation_row,
    settings_section, update_status_row, SettingsDropdownGroupArgs,
};
use labels::{
    assignment_notification_offset_label, calendar_range_label, calendar_view_label,
    current_language_value, language_label, language_options, notification_offset_label,
    theme_mode_label, time_format_label, upcoming_item_limit_options, upcoming_lookahead_label,
    upcoming_lookahead_options,
};

impl KnotQApp {
    pub fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme();
        let mut theme_rows = vec![settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "theme-setting",
                label: tr("settings.appearance.theme_label"),
                dropdown: SettingsDropdown::Theme,
                selected_label: theme_mode_label(self.theme_mode).to_string(),
                options: vec![
                    (
                        tr("settings.appearance.theme_dark").to_string(),
                        ThemeMode::Dark,
                    ),
                    (
                        tr("settings.appearance.theme_light").to_string(),
                        ThemeMode::Light,
                    ),
                    (
                        tr("settings.appearance.theme_system").to_string(),
                        ThemeMode::System,
                    ),
                    (tr("settings.appearance.theme_rose_pine_moon").to_string(), ThemeMode::RosePineMoon),
                    (tr("settings.appearance.theme_catppuccin_mocha").to_string(), ThemeMode::CatppuccinMocha),
                    (tr("settings.appearance.theme_tokyo_night").to_string(), ThemeMode::TokyoNight),
                    (tr("settings.appearance.theme_parchment").to_string(), ThemeMode::Parchment),
                    (tr("settings.appearance.theme_rose_pine_dawn").to_string(), ThemeMode::RosePineDawn),
                    (tr("settings.appearance.theme_catppuccin_latte").to_string(), ThemeMode::CatppuccinLatte),
                ],
                current: self.theme_mode,
                is_open: self.settings_dropdown == Some(SettingsDropdown::Theme),
                t,
            },
            cx,
            |this, mode, cx| this.set_theme_mode(mode, cx),
        )];
        let current_language = current_language_value(self.settings.language.as_deref());
        theme_rows.push(settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "language-setting",
                label: tr("settings.language.label"),
                dropdown: SettingsDropdown::Language,
                selected_label: language_label(current_language).to_string(),
                options: language_options()
                    .into_iter()
                    .map(|(label, value)| (label.to_string(), value))
                    .collect(),
                current: current_language,
                is_open: self.settings_dropdown == Some(SettingsDropdown::Language),
                t,
            },
            cx,
            |this, code, cx| this.set_language(code.map(|c| c.to_string()), cx),
        ));

        let mut calendar_rows = vec![settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "calendar-view-setting",
                label: tr("settings.calendar.view_label"),
                dropdown: SettingsDropdown::CalendarView,
                selected_label: calendar_view_label(self.calendar_view).to_string(),
                options: vec![
                    (
                        tr("settings.calendar.view_week").to_string(),
                        CalendarViewMode::Week,
                    ),
                    (
                        tr("settings.calendar.view_month").to_string(),
                        CalendarViewMode::Month,
                    ),
                ],
                current: self.calendar_view,
                is_open: self.settings_dropdown == Some(SettingsDropdown::CalendarView),
                t,
            },
            cx,
            |this, mode, cx| this.set_calendar_view(mode, cx),
        )];
        calendar_rows.push(settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "calendar-range-setting",
                label: tr("settings.calendar.range_label"),
                dropdown: SettingsDropdown::CalendarRange,
                selected_label: calendar_range_label(self.calendar_week_range).to_string(),
                options: vec![
                    (
                        tr("settings.calendar.range_rolling_week").to_string(),
                        CalendarWeekRange::NextSevenDays,
                    ),
                    (
                        tr("settings.calendar.range_calendar_week").to_string(),
                        CalendarWeekRange::CalendarWeek,
                    ),
                ],
                current: self.calendar_week_range,
                is_open: self.settings_dropdown == Some(SettingsDropdown::CalendarRange),
                t,
            },
            cx,
            |this, range, cx| this.set_calendar_week_range(range, cx),
        ));
        calendar_rows.push(settings_navigation_row(
            "settings-timing-link",
            tr("settings.timing.title"),
            t,
            cx,
            |this, cx| {
                this.settings_showing_timing = true;
                this.settings_dropdown = None;
                cx.notify();
            },
        ));

        let time_rows = vec![settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "time-format-setting",
                label: tr("settings.time.clock_label"),
                dropdown: SettingsDropdown::TimeFormat,
                selected_label: time_format_label(self.time_format).to_string(),
                options: vec![
                    (
                        tr("settings.time.clock_12h").to_string(),
                        TimeFormat::TwelveHour,
                    ),
                    (
                        tr("settings.time.clock_24h").to_string(),
                        TimeFormat::TwentyFourHour,
                    ),
                ],
                current: self.time_format,
                is_open: self.settings_dropdown == Some(SettingsDropdown::TimeFormat),
                t,
            },
            cx,
            |this, format, cx| this.set_time_format(format, cx),
        )];

        let upcoming_display = self.settings.upcoming_display;
        let mut lookahead_rows = Vec::new();
        lookahead_rows.push(settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "event-lookahead-setting",
                label: tr("settings.notifications.events_label"),
                dropdown: SettingsDropdown::EventLookahead,
                selected_label: upcoming_lookahead_label(upcoming_display.event_lookahead_days),
                options: upcoming_lookahead_options(),
                current: upcoming_display.event_lookahead_days,
                is_open: self.settings_dropdown == Some(SettingsDropdown::EventLookahead),
                t,
            },
            cx,
            |this, days, cx| {
                let mut next = this.settings.upcoming_display;
                next.event_lookahead_days = days;
                this.set_upcoming_display_settings(next, cx);
            },
        ));
        lookahead_rows.push(settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "reminder-lookahead-setting",
                label: tr("upcoming.section.reminders"),
                dropdown: SettingsDropdown::ReminderLookahead,
                selected_label: upcoming_lookahead_label(upcoming_display.reminder_lookahead_days),
                options: upcoming_lookahead_options(),
                current: upcoming_display.reminder_lookahead_days,
                is_open: self.settings_dropdown == Some(SettingsDropdown::ReminderLookahead),
                t,
            },
            cx,
            |this, days, cx| {
                let mut next = this.settings.upcoming_display;
                next.reminder_lookahead_days = days;
                this.set_upcoming_display_settings(next, cx);
            },
        ));
        lookahead_rows.push(settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "assignment-lookahead-setting",
                label: tr("upcoming.section.assignments"),
                dropdown: SettingsDropdown::AssignmentLookahead,
                selected_label: upcoming_lookahead_label(
                    upcoming_display.assignment_lookahead_days,
                ),
                options: upcoming_lookahead_options(),
                current: upcoming_display.assignment_lookahead_days,
                is_open: self.settings_dropdown == Some(SettingsDropdown::AssignmentLookahead),
                t,
            },
            cx,
            |this, days, cx| {
                let mut next = this.settings.upcoming_display;
                next.assignment_lookahead_days = days;
                this.set_upcoming_display_settings(next, cx);
            },
        ));
        let mut visibility_rows = vec![settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "maximum-upcoming-items-setting",
                label: tr("settings.display.maximum_items"),
                dropdown: SettingsDropdown::MaximumUpcomingItems,
                selected_label: upcoming_display.maximum_items.to_string(),
                options: upcoming_item_limit_options(),
                current: upcoming_display.maximum_items,
                is_open: self.settings_dropdown == Some(SettingsDropdown::MaximumUpcomingItems),
                t,
            },
            cx,
            |this, maximum_items, cx| {
                let mut next = this.settings.upcoming_display;
                next.maximum_items = maximum_items;
                this.set_upcoming_display_settings(next, cx);
            },
        )];
        visibility_rows.push(choice_row(
            ("show-overdue-setting", 0),
            tr("settings.display.show_overdue"),
            upcoming_display.show_overdue,
            active_marker(upcoming_display.show_overdue, t),
            t,
            cx,
            move |this, cx| {
                let mut next = this.settings.upcoming_display;
                next.show_overdue = !next.show_overdue;
                this.set_upcoming_display_settings(next, cx);
            },
        ));
        visibility_rows.push(choice_row(
            ("show-completed-setting", 0),
            tr("settings.display.show_completed"),
            upcoming_display.show_completed,
            active_marker(upcoming_display.show_completed, t),
            t,
            cx,
            move |this, cx| {
                let mut next = this.settings.upcoming_display;
                next.show_completed = !next.show_completed;
                this.set_upcoming_display_settings(next, cx);
            },
        ));

        let mut notification_rows = vec![settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "event-notification-setting",
                label: tr("settings.notifications.events_label"),
                dropdown: SettingsDropdown::EventNotification,
                selected_label: notification_offset_label(
                    self.notification_defaults.event_offset_secs,
                )
                .to_string(),
                options: vec![
                    (tr("settings.notifications.offset_at_start").to_string(), 0),
                    (
                        tr("settings.notifications.offset_5_min").to_string(),
                        5 * 60,
                    ),
                    (
                        tr("settings.notifications.offset_10_min").to_string(),
                        DEFAULT_EVENT_NOTIFICATION_OFFSET_SECS,
                    ),
                    (
                        tr("settings.notifications.offset_15_min").to_string(),
                        15 * 60,
                    ),
                    (
                        tr("settings.notifications.offset_30_min").to_string(),
                        30 * 60,
                    ),
                    (
                        tr("settings.notifications.offset_1_hr").to_string(),
                        60 * 60,
                    ),
                ],
                current: self.notification_defaults.event_offset_secs,
                is_open: self.settings_dropdown == Some(SettingsDropdown::EventNotification),
                t,
            },
            cx,
            |this, offset_secs, cx| {
                let mut defaults = this.notification_defaults;
                defaults.event_offset_secs = offset_secs;
                this.set_notification_defaults(defaults, cx);
            },
        )];
        notification_rows.push(settings_dropdown_group(
            SettingsDropdownGroupArgs {
                id: "assignment-notification-setting",
                label: tr("settings.notifications.assignments_label"),
                dropdown: SettingsDropdown::AssignmentNotification,
                selected_label: assignment_notification_offset_label(
                    self.notification_defaults.assignment_offset_secs,
                )
                .to_string(),
                options: vec![
                    (tr("settings.notifications.offset_at_due").to_string(), 0),
                    (
                        tr("settings.notifications.offset_1_hr").to_string(),
                        60 * 60,
                    ),
                    (
                        tr("settings.notifications.offset_2_hr").to_string(),
                        2 * 60 * 60,
                    ),
                    (
                        tr("settings.notifications.offset_6_hr").to_string(),
                        6 * 60 * 60,
                    ),
                    (
                        tr("settings.notifications.offset_1_day").to_string(),
                        24 * 60 * 60,
                    ),
                    (
                        tr("settings.notifications.offset_2_days").to_string(),
                        2 * 24 * 60 * 60,
                    ),
                ],
                current: self.notification_defaults.assignment_offset_secs,
                is_open: self.settings_dropdown == Some(SettingsDropdown::AssignmentNotification),
                t,
            },
            cx,
            |this, offset_secs, cx| {
                let mut defaults = this.notification_defaults;
                defaults.assignment_offset_secs = offset_secs;
                this.set_notification_defaults(defaults, cx);
            },
        ));

        if self.settings_showing_timing {
            let subpage_header = div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .pb(px(2.0))
                .child(
                    div()
                        .id("settings-timing-back")
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .hover({
                            let c = t.row_hover;
                            move |h| h.bg(token_rgba(c))
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings_showing_timing = false;
                            this.settings_dropdown = None;
                            cx.notify();
                        }))
                        .child(format!("‹ {}", tr("settings.header.title"))),
                )
                .child(
                    div()
                        .text_size(px(17.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(tr("settings.timing.title")),
                );

            return div()
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
                            .child(subpage_header)
                            .child(settings_section(tr("settings.time.section"), time_rows, t))
                            .child(settings_section(
                                tr("settings.display.upcoming_section"),
                                lookahead_rows,
                                t,
                            ))
                            .child(settings_section(
                                tr("settings.display.visibility_section"),
                                visibility_rows,
                                t,
                            ))
                            .child(settings_section(
                                tr("settings.notifications.section"),
                                notification_rows,
                                t,
                            )),
                    ),
                )
                .into_any_element();
        }

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
