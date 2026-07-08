use chrono::{DateTime, Local, Utc};
use knotq_l10n::{available_locales, t as tr, t_with as tr_with};
use knotq_storage_json::{CalendarViewMode, CalendarWeekRange, ThemeMode, TimeFormat};

pub(super) fn checked_time_label(checked_at: DateTime<Utc>) -> String {
    checked_at.with_timezone(&Local).format("%H:%M").to_string()
}

pub(super) fn theme_mode_label(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::Dark => tr("settings.appearance.theme_dark"),
        ThemeMode::Light => tr("settings.appearance.theme_light"),
        ThemeMode::System => tr("settings.appearance.theme_system"),
    }
}

pub(super) fn calendar_view_label(mode: CalendarViewMode) -> &'static str {
    match mode {
        CalendarViewMode::Week => tr("settings.calendar.view_week"),
        CalendarViewMode::Month => tr("settings.calendar.view_month"),
    }
}

pub(super) fn calendar_range_label(range: CalendarWeekRange) -> &'static str {
    match range {
        CalendarWeekRange::NextSevenDays => tr("settings.calendar.range_rolling_week"),
        CalendarWeekRange::CalendarWeek => tr("settings.calendar.range_calendar_week"),
    }
}

pub(super) fn time_format_label(format: TimeFormat) -> &'static str {
    match format {
        TimeFormat::TwelveHour => tr("settings.time.clock_12h"),
        TimeFormat::TwentyFourHour => tr("settings.time.clock_24h"),
    }
}

pub(super) fn notification_offset_label(offset_secs: i64) -> &'static str {
    match offset_secs {
        0 => tr("settings.notifications.offset_at_start"),
        300 => tr("settings.notifications.offset_5_min"),
        600 => tr("settings.notifications.offset_10_min"),
        900 => tr("settings.notifications.offset_15_min"),
        1_800 => tr("settings.notifications.offset_30_min"),
        3_600 => tr("settings.notifications.offset_1_hr"),
        7_200 => tr("settings.notifications.offset_2_hr"),
        21_600 => tr("settings.notifications.offset_6_hr"),
        86_400 => tr("settings.notifications.offset_1_day"),
        172_800 => tr("settings.notifications.offset_2_days"),
        _ => tr("settings.notifications.offset_custom"),
    }
}

pub(super) fn assignment_notification_offset_label(offset_secs: i64) -> &'static str {
    match offset_secs {
        0 => tr("settings.notifications.offset_at_due"),
        _ => notification_offset_label(offset_secs),
    }
}

pub(super) fn google_calendar_last_synced_label(value: DateTime<Utc>) -> String {
    tr_with(
        "settings.google_calendar.synced_at",
        &[(
            "when",
            &value.with_timezone(&Local).format("%b %-d %H:%M").to_string(),
        )],
    )
}

/// Options for the Language dropdown: "System" first, then every locale from
/// the l10n registry shown by its native (autonym) name. Values are the
/// locale code, or `None` for "follow the OS" (System).
pub(super) fn language_options() -> Vec<(&'static str, Option<&'static str>)> {
    let mut options = vec![(tr("settings.language.system"), None)];
    options.extend(available_locales().iter().map(|l| (l.native, Some(l.code))));
    options
}

/// Resolves the saved `settings.language` code to the matching registry
/// entry's `'static` code, so it can be compared against `language_options`'
/// values. Unrecognized/absent codes fall back to `None` ("System").
pub(super) fn current_language_value(language: Option<&str>) -> Option<&'static str> {
    let code = language?;
    available_locales()
        .iter()
        .find(|l| l.code == code)
        .map(|l| l.code)
}

pub(super) fn language_label(current: Option<&'static str>) -> &'static str {
    match current {
        None => tr("settings.language.system"),
        Some(code) => available_locales()
            .iter()
            .find(|l| l.code == code)
            .map(|l| l.native)
            .unwrap_or_else(|| tr("settings.language.system")),
    }
}
