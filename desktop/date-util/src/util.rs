use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};

pub fn local_now() -> DateTime<Local> {
    Local::now()
}

pub fn utc_to_local(dt: DateTime<Utc>) -> DateTime<Local> {
    dt.with_timezone(&Local)
}

pub fn start_of_week(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday().into())
}

pub fn end_of_week(date: NaiveDate) -> NaiveDate {
    start_of_week(date) + Duration::days(7)
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

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();
    (first_next - Duration::days(1)).day()
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
