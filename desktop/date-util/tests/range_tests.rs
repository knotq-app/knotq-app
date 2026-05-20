use chrono::NaiveDate;
use knotq_date_util::week_range;

#[test]
fn week_range_starts_on_monday() {
    let range = week_range(0, NaiveDate::from_ymd_opt(2026, 5, 18).unwrap());
    assert_eq!(
        range.start.date_naive(),
        NaiveDate::from_ymd_opt(2026, 5, 18).unwrap()
    );
    assert_eq!(
        range.end.date_naive(),
        NaiveDate::from_ymd_opt(2026, 5, 25).unwrap()
    );
}
