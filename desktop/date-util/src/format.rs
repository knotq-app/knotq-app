use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use knotq_model::TimeFormat;
use std::fmt::Display;

pub fn format_time<Tz>(format: TimeFormat, dt: DateTime<Tz>) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    match format {
        TimeFormat::TwelveHour => dt.format("%-I:%M %p").to_string(),
        TimeFormat::TwentyFourHour => dt.format("%H:%M").to_string(),
    }
}

pub fn format_hour_label(format: TimeFormat, hour: u32) -> String {
    let hour = hour % 24;
    match format {
        TimeFormat::TwelveHour => {
            let suffix = if hour < 12 { "AM" } else { "PM" };
            let h = match hour % 12 {
                0 => 12,
                other => other,
            };
            format!("{h} {suffix}")
        }
        TimeFormat::TwentyFourHour => format!("{hour:02}:00"),
    }
}

pub fn format_date(date: NaiveDate) -> String {
    format!("{} {}, {}", date.format("%b"), date.day(), date.year())
}

pub fn format_contextual_date(date: NaiveDate, reference_year: i32) -> String {
    let base = format!("{} {}", date.format("%B"), date.day());
    if date.year() == reference_year {
        base
    } else {
        format!("{base}, {}", date.year())
    }
}

pub fn format_datetime(format: TimeFormat, dt: DateTime<Utc>) -> String {
    let local = dt.with_timezone(&Local);
    format!(
        "{} {}",
        format_date(local.date_naive()),
        format_time(format, local)
    )
}

pub fn format_date_time(format: TimeFormat, dt: DateTime<Local>) -> String {
    format!("{} {}", dt.format("%a %b %d"), format_time(format, dt))
}

pub fn format_contextual_datetime(
    format: TimeFormat,
    dt: DateTime<Local>,
    reference_year: i32,
) -> String {
    format!(
        "{} {}",
        format_contextual_date(dt.date_naive(), reference_year),
        format_time(format, dt)
    )
}

pub fn format_month_header(year: i32, month: u32) -> String {
    NaiveDate::from_ymd_opt(year, month, 1)
        .map(|date| date.format("%B %Y").to_string())
        .unwrap_or_else(|| format!("{year}-{month:02}"))
}

pub fn format_relative(date: NaiveDate, today: NaiveDate) -> String {
    let delta = (date - today).num_days();
    match delta {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        d if d > 1 => format!("In {d} days"),
        d => format!("{} days ago", -d),
    }
}

pub fn format_duration(duration: Duration) -> String {
    let secs = duration.num_seconds().abs();
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}
