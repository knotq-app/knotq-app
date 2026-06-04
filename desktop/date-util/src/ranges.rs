use crate::util::start_of_week;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};

pub const UPCOMING_HORIZON_DAYS: i64 = 14;
pub const UPCOMING_LIMIT: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DateRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl DateRange {
    pub fn contains(&self, dt: DateTime<Utc>) -> bool {
        dt >= self.start && dt < self.end
    }

    pub fn intersects(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> bool {
        start < self.end && end > self.start
    }
}

pub fn week_range(offset: i32, today: NaiveDate) -> DateRange {
    let week_start = start_of_week(today) + Duration::weeks(offset.into());
    DateRange {
        start: utc_midnight(week_start),
        end: utc_midnight(week_start + Duration::days(7)),
    }
}

pub fn day_range(date: NaiveDate) -> DateRange {
    DateRange {
        start: utc_midnight(date),
        end: utc_midnight(date + Duration::days(1)),
    }
}

pub fn upcoming_range(from: DateTime<Utc>) -> DateRange {
    DateRange {
        start: from,
        end: from + Duration::days(UPCOMING_HORIZON_DAYS),
    }
}

pub fn month_range(year: i32, month: u32) -> DateRange {
    let start = NaiveDate::from_ymd_opt(year, month, 1)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, 1, 1).unwrap());
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();
    DateRange {
        start: utc_midnight(start),
        end: utc_midnight(end),
    }
}

fn utc_midnight(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
}
