use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement};
use gpui_component::scroll::ScrollableElement as _;
use knotq_storage_json::{CalendarViewMode, NotificationDefaults, ThemeMode, TimeFormat};

use crate::app::KnotQApp;
use crate::theme_gpui::{all_themes, token_hsla, token_rgba};

impl KnotQApp {
    pub fn render_settings(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme();
        let themes = all_themes();
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for (i, (label, mode, theme)) in [
            ("System", ThemeMode::System, self.theme()),
            ("Dark", ThemeMode::Dark, themes[0]),
            ("Light", ThemeMode::Light, themes[1]),
        ]
        .into_iter()
        .enumerate()
        {
            let is_active = self.theme_mode == mode;
            rows.push(
                div()
                    .id(("theme", i))
                    .px(px(16.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .when(is_active, {
                        let c = t.row_selected;
                        move |s| s.bg(token_rgba(c))
                    })
                    .when(!is_active, {
                        let c = t.row_hover;
                        move |s| s.hover(move |h| h.bg(token_rgba(c)))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.set_theme_mode(mode, cx);
                    }))
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(3.0))
                            .bg(token_rgba(theme.bg_app))
                            .border_1()
                            .border_color(token_rgba(theme.border_main)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(token_hsla(t.text_primary))
                            .child(label),
                    )
                    .into_any_element(),
            );
        }

        let mut calendar_rows: Vec<gpui::AnyElement> = Vec::new();
        for (idx, (label, mode)) in [
            ("Week", CalendarViewMode::Week),
            ("Month", CalendarViewMode::Month),
        ]
        .into_iter()
        .enumerate()
        {
            let is_active = self.calendar_view == mode;
            calendar_rows.push(
                div()
                    .id(("calendar-setting", idx))
                    .px(px(16.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .when(is_active, {
                        let c = t.row_selected;
                        move |s| s.bg(token_rgba(c))
                    })
                    .when(!is_active, {
                        let c = t.row_hover;
                        move |s| s.hover(move |h| h.bg(token_rgba(c)))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.set_calendar_view(mode, cx);
                    }))
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(3.0))
                            .bg(token_rgba(if is_active {
                                t.text_today
                            } else {
                                t.button_bg
                            }))
                            .border_1()
                            .border_color(token_rgba(t.border_main)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(token_hsla(t.text_primary))
                            .child(label),
                    )
                    .into_any_element(),
            );
        }

        let mut time_rows: Vec<gpui::AnyElement> = Vec::new();
        for (idx, (label, format)) in [
            ("12-hour", TimeFormat::TwelveHour),
            ("24-hour", TimeFormat::TwentyFourHour),
        ]
        .into_iter()
        .enumerate()
        {
            let is_active = self.time_format == format;
            time_rows.push(
                div()
                    .id(("time-format-setting", idx))
                    .px(px(16.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .when(is_active, {
                        let c = t.row_selected;
                        move |s| s.bg(token_rgba(c))
                    })
                    .when(!is_active, {
                        let c = t.row_hover;
                        move |s| s.hover(move |h| h.bg(token_rgba(c)))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.set_time_format(format, cx);
                    }))
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(3.0))
                            .bg(token_rgba(if is_active {
                                t.text_today
                            } else {
                                t.button_bg
                            }))
                            .border_1()
                            .border_color(token_rgba(t.border_main)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(token_hsla(t.text_primary))
                            .child(label),
                    )
                    .into_any_element(),
            );
        }

        let mut notification_rows: Vec<gpui::AnyElement> = Vec::new();
        if let Some(err) = &self.notification_error {
            notification_rows.push(
                div()
                    .px(px(16.0))
                    .py(px(8.0))
                    .mx(px(16.0))
                    .rounded(px(6.0))
                    .bg(token_rgba(t.text_today))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(token_hsla(t.bg_app))
                            .child(err.clone()),
                    )
                    .into_any_element(),
            );
        }
        if let Some(status) = &self.notification_status {
            notification_rows.push(
                div()
                    .px(px(16.0))
                    .py(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(token_hsla(t.text_dim))
                            .child(status.clone()),
                    )
                    .into_any_element(),
            );
        }
        notification_rows.push(
            div()
                .id("test-notification-btn")
                .mx(px(16.0))
                .my(px(6.0))
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .bg(token_rgba(t.button_bg))
                .cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.send_test_notification(cx);
                }))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(token_hsla(t.text_primary))
                        .child("Send Test (Immediate)"),
                )
                .into_any_element(),
        );
        notification_rows.push(
            div()
                .id("test-scheduled-notification-btn")
                .mx(px(16.0))
                .my(px(6.0))
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .bg(token_rgba(t.button_bg))
                .cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.send_scheduled_test_notification(cx);
                }))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(token_hsla(t.text_primary))
                        .child("Send Test (Scheduled 15s)"),
                )
                .into_any_element(),
        );
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
            notification_rows.push(notification_setting_row(
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
            notification_rows.push(notification_setting_row(
                ("assignment-notification-setting", idx),
                label,
                is_active,
                defaults,
                t,
                cx,
            ));
        }

        div()
            .flex_1()
            .h_full()
            .bg(token_hsla(t.bg_app))
            .pt(px(16.0))
            .overflow_y_scrollbar()
            .child(
                div()
                    .px(px(16.0))
                    .pb(px(8.0))
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_primary))
                    .child("Theme"),
            )
            .children(rows)
            .child(
                div()
                    .px(px(16.0))
                    .pt(px(18.0))
                    .pb(px(8.0))
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_primary))
                    .child("Calendar"),
            )
            .children(calendar_rows)
            .child(
                div()
                    .px(px(16.0))
                    .pt(px(18.0))
                    .pb(px(8.0))
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_primary))
                    .child("Time"),
            )
            .children(time_rows)
            .child(
                div()
                    .px(px(16.0))
                    .pt(px(18.0))
                    .pb(px(8.0))
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_primary))
                    .child("Notifications"),
            )
            .children(notification_rows)
            .into_any_element()
    }
}

fn settings_subheading(label: &'static str, t: crate::theme_gpui::Theme) -> gpui::AnyElement {
    div()
        .px(px(16.0))
        .pt(px(6.0))
        .pb(px(3.0))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(token_hsla(t.text_dim))
        .child(label)
        .into_any_element()
}

fn notification_setting_row(
    id: (&'static str, usize),
    label: &'static str,
    is_active: bool,
    defaults: NotificationDefaults,
    t: crate::theme_gpui::Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id(id)
        .px(px(16.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .when(is_active, {
            let c = t.row_selected;
            move |s| s.bg(token_rgba(c))
        })
        .when(!is_active, {
            let c = t.row_hover;
            move |s| s.hover(move |h| h.bg(token_rgba(c)))
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.set_notification_defaults(defaults, cx);
        }))
        .child(
            div()
                .w(px(14.0))
                .h(px(14.0))
                .rounded(px(3.0))
                .bg(token_rgba(if is_active {
                    t.text_today
                } else {
                    t.button_bg
                }))
                .border_1()
                .border_color(token_rgba(t.border_main)),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(token_hsla(t.text_primary))
                .child(label),
        )
        .into_any_element()
}
