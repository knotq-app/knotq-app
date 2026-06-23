use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};

pub fn start_of_week(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday().into())
}

pub fn local_date_repeat_until_utc(date: NaiveDate) -> Option<DateTime<Utc>> {
    let local_end = date.and_hms_opt(23, 59, 59)?;
    Local
        .from_local_datetime(&local_end)
        .latest()
        .map(|dt| dt.with_timezone(&Utc))
}

pub fn add_months_exact(dt: DateTime<Utc>, months: i32) -> DateTime<Utc> {
    let naive = dt.naive_utc();
    let date = naive.date();
    let total_months = date.year() * 12 + date.month0() as i32 + months;
    let year = total_months.div_euclid(12);
    let month0 = total_months.rem_euclid(12) as u32;
    let month = month0 + 1;
    let day = date.day().min(days_in_month(year, month));
    let new_date = NaiveDate::from_ymd_opt(year, month, day).unwrap();
    Utc.from_utc_datetime(&new_date.and_time(naive.time()))
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();
    (first_next - Duration::days(1)).day()
}

pub fn prev_month(date: NaiveDate) -> NaiveDate {
    let (y, m) = if date.month() == 1 {
        (date.year() - 1, 12u32)
    } else {
        (date.year(), date.month() - 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(date)
}

pub fn next_month(date: NaiveDate) -> NaiveDate {
    let (y, m) = if date.month() == 12 {
        (date.year() + 1, 1u32)
    } else {
        (date.year(), date.month() + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(date)
}

pub fn month_start(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date)
}

/// Converts a date + fractional hour into a UTC datetime, snapped to 15-minute increments.
pub fn snapped_calendar_datetime(date: NaiveDate, hour: f32) -> DateTime<Utc> {
    let total_minutes = (hour * 60.0).round() as i64;
    let snapped = ((total_minutes + 7) / 15 * 15).clamp(0, 24 * 60);
    let day = date + Duration::days(snapped / (24 * 60));
    let minute_of_day = snapped % (24 * 60);
    let h = (minute_of_day / 60) as u32;
    let m = (minute_of_day % 60) as u32;
    Local
        .with_ymd_and_hms(day.year(), day.month(), day.day(), h, m, 0)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_months_clamps_month_end() {
        let dt = Utc.with_ymd_and_hms(2024, 1, 31, 9, 0, 0).unwrap();
        assert_eq!(add_months_exact(dt, 1).date_naive().day(), 29);
        assert_eq!(add_months_exact(dt, 2).date_naive().day(), 31);
    }
}
