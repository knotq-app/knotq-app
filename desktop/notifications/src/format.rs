use chrono::{DateTime, Local, Timelike, Utc};
use knotq_model::Item;

use crate::NotificationKind;

pub(crate) fn title_for(item: &Item) -> String {
    let text = item.text();
    let title = text.lines().next().unwrap_or("").trim();
    if title.is_empty() {
        "(untitled)".to_string()
    } else {
        title.to_string()
    }
}

pub(crate) fn body_for(
    kind: NotificationKind,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> String {
    match kind {
        NotificationKind::Reminder => start
            .map(|dt| format!("At {}", date_time_label(dt)))
            .unwrap_or_default(),
        NotificationKind::Assignment => end
            .map(|dt| format!("Due {}", date_time_label(dt)))
            .unwrap_or_default(),
        NotificationKind::Event => event_body(start, end),
    }
}

fn event_body(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> String {
    match (start, end) {
        (Some(start), Some(end)) => {
            let start_local = start.with_timezone(&Local);
            let end_local = end.with_timezone(&Local);
            if start_local.date_naive() == end_local.date_naive() {
                format!(
                    "{}, {}",
                    day_label(start_local),
                    time_range_label(start_local, end_local)
                )
            } else {
                format!("{} to {}", date_time_label(start), date_time_label(end))
            }
        }
        (Some(start), None) => date_time_label(start),
        (None, Some(end)) => format!("Until {}", date_time_label(end)),
        _ => String::new(),
    }
}

fn date_time_label(dt: DateTime<Utc>) -> String {
    let local = dt.with_timezone(&Local);
    let day = day_label(local);
    let time = time_label(local);
    format!("{day}, {time}")
}

fn day_label(dt: DateTime<Local>) -> String {
    dt.format("%a").to_string()
}

fn time_range_label(start: DateTime<Local>, end: DateTime<Local>) -> String {
    if meridiem(start) == meridiem(end) {
        format!(
            "{} to {} {}",
            clock_label(start),
            clock_label(end),
            meridiem(end)
        )
    } else {
        format!("{} to {}", time_label(start), time_label(end))
    }
}

fn time_label(dt: DateTime<Local>) -> String {
    let suffix = meridiem(dt);
    format!("{} {}", clock_label(dt), suffix)
}

fn clock_label(dt: DateTime<Local>) -> String {
    let hour = match dt.hour() % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{}:{:02}", hour, dt.minute())
}

fn meridiem(dt: DateTime<Local>) -> &'static str {
    if dt.hour() < 12 {
        "AM"
    } else {
        "PM"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn event_body_omits_from_and_repeated_meridiem_for_same_period() {
        let start = local_datetime(2026, 5, 21, 13, 0);
        let end = start + Duration::minutes(75);

        let body = body_for(
            NotificationKind::Event,
            Some(start.with_timezone(&Utc)),
            Some(end.with_timezone(&Utc)),
        );

        assert_eq!(body, "Thu, 1:00 to 2:15 PM");
    }

    #[test]
    fn event_body_keeps_both_meridiems_when_range_crosses_periods() {
        let start = local_datetime(2026, 5, 21, 11, 30);
        let end = start + Duration::hours(1);

        let body = body_for(
            NotificationKind::Event,
            Some(start.with_timezone(&Utc)),
            Some(end.with_timezone(&Utc)),
        );

        assert_eq!(body, "Thu, 11:30 AM to 12:30 PM");
    }

    #[test]
    fn reminder_body_has_single_comma_between_day_and_time() {
        let start = local_datetime(2026, 5, 22, 0, 41);

        let body = body_for(
            NotificationKind::Reminder,
            Some(start.with_timezone(&Utc)),
            None,
        );

        assert_eq!(body, "At Fri, 12:41 AM");
    }

    fn local_datetime(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("test datetime should resolve in the local timezone")
    }
}
