use chrono::{DateTime, Local, Timelike, Utc};
use knotq_model::Item;

use crate::NotificationKind;

pub(crate) fn title_for(item: &Item) -> String {
    let title = item.text.lines().next().unwrap_or("").trim();
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
                    "From {} to {}",
                    date_time_label(start),
                    time_label(end_local)
                )
            } else {
                format!(
                    "From {} to {}",
                    date_time_label(start),
                    date_time_label(end)
                )
            }
        }
        (Some(start), None) => format!("At {}", date_time_label(start)),
        (None, Some(end)) => format!("Until {}", date_time_label(end)),
        _ => String::new(),
    }
}

fn date_time_label(dt: DateTime<Utc>) -> String {
    let local = dt.with_timezone(&Local);
    let day = local.format("%a").to_string();
    let time = time_label(local);
    format!("{day}, {time}")
}

fn time_label(dt: DateTime<Local>) -> String {
    let suffix = if dt.hour() < 12 { "AM" } else { "PM" };
    let hour = match dt.hour() % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{}:{:02} {}", hour, dt.minute(), suffix)
}
