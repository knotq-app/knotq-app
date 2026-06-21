use chrono::{DateTime, Local, Utc};
use knotq_storage_json::{CalendarViewMode, CalendarWeekRange, ThemeMode, TimeFormat};

pub(super) fn checked_time_label(checked_at: DateTime<Utc>) -> String {
    checked_at.with_timezone(&Local).format("%H:%M").to_string()
}

pub(super) fn theme_mode_label(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::Dark => "Dark",
        ThemeMode::Light => "Light",
        ThemeMode::System => "System",
    }
}

pub(super) fn calendar_view_label(mode: CalendarViewMode) -> &'static str {
    match mode {
        CalendarViewMode::Week => "Week",
        CalendarViewMode::Month => "Month",
    }
}

pub(super) fn calendar_range_label(range: CalendarWeekRange) -> &'static str {
    match range {
        CalendarWeekRange::NextSevenDays => "Rolling week",
        CalendarWeekRange::CalendarWeek => "Calendar week",
    }
}

pub(super) fn time_format_label(format: TimeFormat) -> &'static str {
    match format {
        TimeFormat::TwelveHour => "12-hour",
        TimeFormat::TwentyFourHour => "24-hour",
    }
}

pub(super) fn notification_offset_label(offset_secs: i64) -> &'static str {
    match offset_secs {
        0 => "At start",
        300 => "5 min",
        600 => "10 min",
        900 => "15 min",
        1_800 => "30 min",
        3_600 => "1 hr",
        7_200 => "2 hr",
        21_600 => "6 hr",
        86_400 => "1 day",
        172_800 => "2 days",
        _ => "Custom",
    }
}

pub(super) fn assignment_notification_offset_label(offset_secs: i64) -> &'static str {
    match offset_secs {
        0 => "At due",
        _ => notification_offset_label(offset_secs),
    }
}

pub(super) fn google_calendar_last_synced_label(value: DateTime<Utc>) -> String {
    format!(
        "Synced {}",
        value.with_timezone(&Local).format("%b %-d %H:%M")
    )
}
