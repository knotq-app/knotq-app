use chrono::{NaiveDate, TimeZone, Utc};
use knotq_date_util::{format_contextual_date, format_hour_label, format_time};
use knotq_model::TimeFormat;

#[test]
fn time_formats_are_centralized() {
    let dt = Utc.with_ymd_and_hms(2026, 1, 1, 13, 5, 0).unwrap();
    assert_eq!(format_time(TimeFormat::TwentyFourHour, dt), "13:05");
    assert_eq!(format_hour_label(TimeFormat::TwelveHour, 0), "12 AM");
}

#[test]
fn contextual_dates_omit_reference_year() {
    let date = NaiveDate::from_ymd_opt(2026, 5, 18).unwrap();
    assert_eq!(format_contextual_date(date, 2026), "May 18");
    assert_eq!(format_contextual_date(date, 2025), "May 18, 2026");
}
