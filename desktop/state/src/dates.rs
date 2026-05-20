use chrono::{Datelike, NaiveDate};

pub fn add_months(date: NaiveDate, offset: i32) -> NaiveDate {
    let month_index = date.year() * 12 + date.month0() as i32 + offset;
    let year = month_index.div_euclid(12);
    let month0 = month_index.rem_euclid(12);
    let month = (month0 + 1) as u32;
    let day = date.day().min(days_in_month(year, month));
    NaiveDate::from_ymd_opt(year, month, day).unwrap_or(date)
}

pub fn calendar_month_keys_between(start: NaiveDate, end: NaiveDate) -> Vec<(i32, u32)> {
    let first = start.min(end).with_day(1).unwrap_or(start.min(end));
    let last = end.max(start).with_day(1).unwrap_or(end.max(start));
    let mut months = Vec::new();
    let mut current = first;
    loop {
        months.push((current.year(), current.month()));
        if current == last {
            break;
        }
        current = add_months(current, 1);
    }
    months
}

pub fn daily_queue_initial_start(today: NaiveDate) -> NaiveDate {
    let current_month_start = today.with_day(1).unwrap_or(today);
    add_months(current_month_start, -1)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|date| date.pred_opt())
        .map(|date| date.day())
        .unwrap_or(31)
}
