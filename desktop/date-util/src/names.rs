//! Localized weekday/month names. `chrono`'s `%a`/`%A`/`%b`/`%B` strftime
//! specifiers always render English, so any render site that needs a
//! human-visible weekday or month name should go through these helpers
//! instead so the catalog (`knotq-l10n`) can translate them.

use chrono::Weekday;

/// Full weekday name ("Monday" .. "Sunday").
pub fn weekday_name(weekday: Weekday) -> &'static str {
    knotq_l10n::t(match weekday {
        Weekday::Mon => "common.weekday.monday",
        Weekday::Tue => "common.weekday.tuesday",
        Weekday::Wed => "common.weekday.wednesday",
        Weekday::Thu => "common.weekday.thursday",
        Weekday::Fri => "common.weekday.friday",
        Weekday::Sat => "common.weekday.saturday",
        Weekday::Sun => "common.weekday.sunday",
    })
}

/// Single-letter weekday initial derived from the localized short name
/// ("M" for Monday, etc.), for compact calendar-grid day headers.
pub fn weekday_name_initial(weekday: Weekday) -> String {
    weekday_short_name(weekday)
        .chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_default()
}

/// Short weekday name ("Mon" .. "Sun").
pub fn weekday_short_name(weekday: Weekday) -> &'static str {
    knotq_l10n::t(match weekday {
        Weekday::Mon => "common.weekday_short.mon",
        Weekday::Tue => "common.weekday_short.tue",
        Weekday::Wed => "common.weekday_short.wed",
        Weekday::Thu => "common.weekday_short.thu",
        Weekday::Fri => "common.weekday_short.fri",
        Weekday::Sat => "common.weekday_short.sat",
        Weekday::Sun => "common.weekday_short.sun",
    })
}

/// Full month name ("January" .. "December") for a 1-based month number.
/// Out-of-range values fall back to January rather than panicking.
pub fn month_name(month: u32) -> &'static str {
    knotq_l10n::t(match month {
        1 => "common.month.january",
        2 => "common.month.february",
        3 => "common.month.march",
        4 => "common.month.april",
        5 => "common.month.may",
        6 => "common.month.june",
        7 => "common.month.july",
        8 => "common.month.august",
        9 => "common.month.september",
        10 => "common.month.october",
        11 => "common.month.november",
        12 => "common.month.december",
        _ => "common.month.january",
    })
}

/// Short month name ("Jan" .. "Dec") for a 1-based month number.
/// Out-of-range values fall back to January rather than panicking.
pub fn month_short_name(month: u32) -> &'static str {
    knotq_l10n::t(match month {
        1 => "common.month_short.jan",
        2 => "common.month_short.feb",
        3 => "common.month_short.mar",
        4 => "common.month_short.apr",
        5 => "common.month_short.may",
        6 => "common.month_short.jun",
        7 => "common.month_short.jul",
        8 => "common.month_short.aug",
        9 => "common.month_short.sep",
        10 => "common.month_short.oct",
        11 => "common.month_short.nov",
        12 => "common.month_short.dec",
        _ => "common.month_short.jan",
    })
}
