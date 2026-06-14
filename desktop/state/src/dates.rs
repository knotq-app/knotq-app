use chrono::{Datelike, Duration, NaiveDate};

/// Number of days (today inclusive) the daily queue renders by default before
/// older days are paged in lazily on scroll. Kept to two weeks so the scroll
/// view only builds/lays out a handful of editors per frame instead of the
/// whole loaded month.
pub const DAILY_QUEUE_DEFAULT_WINDOW_DAYS: i64 = 14;

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

/// Start of the daily-queue *render* window: the last [`DAILY_QUEUE_DEFAULT_WINDOW_DAYS`]
/// days up to and including `today`. Older days stay indexed and are paged in
/// lazily as the user scrolls toward the top, so this only governs how much is
/// drawn on open — not what is available. Kept separate from
/// [`daily_queue_initial_start`], which still preloads a wider span from disk so
/// the calendar and the first scroll-back have their data ready in memory.
pub fn daily_queue_default_window_start(today: NaiveDate) -> NaiveDate {
    today - Duration::days(DAILY_QUEUE_DEFAULT_WINDOW_DAYS - 1)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_render_window_is_two_weeks_inclusive() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 14).unwrap();
        let start = daily_queue_default_window_start(today);
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 6, 1).unwrap());
        // today inclusive => exactly DAILY_QUEUE_DEFAULT_WINDOW_DAYS days rendered.
        assert_eq!(
            (today - start).num_days() + 1,
            DAILY_QUEUE_DEFAULT_WINDOW_DAYS
        );
    }

    #[test]
    fn default_render_window_crosses_month_boundary() {
        // Unlike daily_queue_initial_start (which snaps to a month edge), the
        // render window is a fixed rolling span that spills into the prior month.
        let today = NaiveDate::from_ymd_opt(2026, 3, 5).unwrap();
        assert_eq!(
            daily_queue_default_window_start(today),
            NaiveDate::from_ymd_opt(2026, 2, 20).unwrap()
        );
    }
}
