use super::*;

pub(super) fn hour_12(hour: u32) -> u32 {
    match hour % 12 {
        0 => 12,
        hour => hour,
    }
}

pub(super) fn parse_popover_hour(time_format: TimeFormat, hour: &str, is_pm: bool) -> Option<u32> {
    match time_format {
        TimeFormat::TwentyFourHour => {
            if hour.is_empty() || hour.len() > 2 {
                return None;
            }
            hour.parse::<u32>().ok().filter(|hour| *hour <= 23)
        }
        TimeFormat::TwelveHour => {
            if hour.is_empty() || hour.len() > 2 {
                return None;
            }
            let hour = hour.parse::<u32>().ok()?;
            if !(1..=12).contains(&hour) {
                return None;
            }
            Some(match (hour, is_pm) {
                (12, false) => 0,
                (12, true) => 12,
                (_, false) => hour,
                (_, true) => hour + 12,
            })
        }
    }
}

pub(super) fn local_dt_from_parts(
    date: NaiveDate,
    hour: u32,
    minute: u32,
) -> chrono::DateTime<Utc> {
    Local
        .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0)
        .single()
        .unwrap_or_else(Local::now)
        .with_timezone(&Utc)
}

pub(super) fn rounded_local_now_utc() -> chrono::DateTime<Utc> {
    let now = Local::now();
    let rounded = ((now.timestamp() + 899) / 900) * 900;
    Local
        .timestamp_opt(rounded, 0)
        .single()
        .unwrap_or(now)
        .with_timezone(&Utc)
}

pub(super) fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let Some(first_next) = NaiveDate::from_ymd_opt(next_year, next_month, 1) else {
        return 30;
    };
    (first_next - Duration::days(1)).day()
}

// ── Date popover helpers ──────────────────────────────────────────────────

pub(crate) fn default_datetime(kind: DateKind, item: &Item) -> chrono::DateTime<Utc> {
    match kind {
        DateKind::Start => item
            .start
            .or_else(|| item.end.map(|dt| local_day_time_utc(dt, 9, 0)))
            .unwrap_or_else(default_start_now_utc),
        DateKind::End => item
            .end
            .or_else(|| item.start.map(|dt| dt + Duration::hours(1)))
            .unwrap_or_else(default_end_today_utc),
        DateKind::Available => item.available.unwrap_or_else(rounded_local_now_utc),
    }
}

pub(super) fn default_start_now_utc() -> chrono::DateTime<Utc> {
    let raw = Local::now() + Duration::minutes(15);
    Local
        .with_ymd_and_hms(
            raw.year(),
            raw.month(),
            raw.day(),
            raw.hour(),
            raw.minute(),
            0,
        )
        .earliest()
        .unwrap_or(raw)
        .with_timezone(&Utc)
}

pub(super) fn default_end_today_utc() -> chrono::DateTime<Utc> {
    let now = Local::now();
    Local
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 23, 0, 0)
        .earliest()
        .unwrap_or(now)
        .with_timezone(&Utc)
}

pub(super) fn local_day_time_utc(
    reference: chrono::DateTime<Utc>,
    hour: u32,
    minute: u32,
) -> chrono::DateTime<Utc> {
    let local = reference.with_timezone(&Local);
    Local
        .with_ymd_and_hms(local.year(), local.month(), local.day(), hour, minute, 0)
        .earliest()
        .unwrap_or(local)
        .with_timezone(&Utc)
}

pub(crate) fn parse_popover_datetime(
    time_format: TimeFormat,
    year: &str,
    month: &str,
    day: &str,
    hour: &str,
    minute: &str,
    hour_is_pm: bool,
) -> Option<chrono::DateTime<Utc>> {
    let year = year.trim();
    if year.len() != 4 {
        return None;
    }
    let month = month.trim();
    let day = day.trim();
    let hour = hour.trim();
    let minute = minute.trim();
    if month.len() != 2 || day.len() != 2 || minute.is_empty() || minute.len() > 2 {
        return None;
    }
    let year = year.parse::<i32>().ok()?;
    let month = month.parse::<u32>().ok()?;
    let day = day.parse::<u32>().ok()?;
    let hour = parse_popover_hour(time_format, hour, hour_is_pm)?;
    let minute = minute.parse::<u32>().ok()?;
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    Local
        .with_ymd_and_hms(
            date.year(),
            date.month(),
            date.day(),
            time.hour(),
            time.minute(),
            0,
        )
        .single()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_popover_datetime_accepts_single_digit_minutes() {
        let parsed = parse_popover_datetime(
            TimeFormat::TwentyFourHour,
            "2026",
            "05",
            "18",
            "09",
            "1",
            false,
        )
        .unwrap()
        .with_timezone(&Local);

        assert_eq!(parsed.minute(), 1);
    }

    #[test]
    fn parse_popover_datetime_rejects_empty_or_long_minutes() {
        assert!(parse_popover_datetime(
            TimeFormat::TwentyFourHour,
            "2026",
            "05",
            "18",
            "09",
            "",
            false,
        )
        .is_none());
        assert!(parse_popover_datetime(
            TimeFormat::TwentyFourHour,
            "2026",
            "05",
            "18",
            "09",
            "123",
            false,
        )
        .is_none());
    }
}
