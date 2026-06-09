use super::*;

pub(super) fn format_optional_datetime(
    time_format: TimeFormat,
    dt: Option<DateTime<Utc>>,
    use_relative_day: bool,
) -> String {
    dt.map(|d| {
        let local = d.with_timezone(&Local);
        if use_relative_day {
            format_relative_datetime(time_format, local)
        } else {
            format_date_time(time_format, local)
        }
    })
    .unwrap_or_else(|| "None".to_string())
}

fn format_relative_datetime(time_format: TimeFormat, dt: DateTime<Local>) -> String {
    let today = Local::now().date_naive();
    let date = dt.date_naive();
    let day = if date == today {
        "Today".to_string()
    } else if date == today + Duration::days(1) {
        "Tomorrow".to_string()
    } else if date == today - Duration::days(1) {
        "Yesterday".to_string()
    } else if date > today && date < today + Duration::days(7) {
        dt.format("%a").to_string()
    } else if date.year() == today.year() {
        dt.format("%b %d").to_string()
    } else {
        dt.format("%Y %b %d").to_string()
    };
    format!("{day} {}", format_time(time_format, dt))
}

pub(super) fn format_lead_time(
    time_format: TimeFormat,
    offset_secs: i64,
    trigger_at: Option<DateTime<Utc>>,
) -> String {
    if offset_secs == 0 {
        return "At time".to_string();
    }
    if offset_secs < 0 {
        if let Some(trigger_at) = trigger_at {
            let fire_at = trigger_at - Duration::seconds(offset_secs);
            return format_relative_datetime(time_format, fire_at.with_timezone(&Local));
        }
    }
    let suffix = if offset_secs > 0 { "before" } else { "after" };
    format!("{} {suffix}", format_duration(offset_secs.abs()))
}

pub(super) fn notification_trigger_at(
    kind: ItemKind,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match kind {
        ItemKind::Reminder | ItemKind::Event => start,
        ItemKind::Assignment => end,
        ItemKind::Procedure => None,
    }
}

fn format_duration(seconds: i64) -> String {
    let days = seconds / 86_400;
    if days > 0 && seconds % 86_400 == 0 {
        return plural(days, "day");
    }
    let hours = seconds / 3_600;
    if hours > 0 && seconds % 3_600 == 0 {
        return plural(hours, "hour");
    }
    let minutes = seconds / 60;
    if minutes > 0 {
        return plural(minutes, "minute");
    }
    plural(seconds, "second")
}

fn plural(value: i64, unit: &str) -> String {
    if value == 1 {
        format!("1 {unit}")
    } else {
        format!("{value} {unit}s")
    }
}

pub(super) fn repeat_summary(recurrence: Option<&Recurrence>, time_format: TimeFormat) -> String {
    let Some(recurrence) = recurrence else {
        return "None".to_string();
    };
    if let Some(simple) = editable_simple_recurrence(recurrence) {
        return simple_repeat_summary(&simple, time_format);
    }
    let mut parts = Vec::new();
    if !recurrence.rrules.is_empty() {
        parts.push(format!(
            "RRULE {}",
            truncate(&recurrence.rrules.join(", "), 72)
        ));
    }
    if !recurrence.rdates.is_empty() {
        parts.push(format!("{} extra date(s)", recurrence.rdates.len()));
    }
    if !recurrence.exdates.is_empty() {
        parts.push(format!("{} exception date(s)", recurrence.exdates.len()));
    }
    if !recurrence.overrides.is_empty() {
        parts.push(format!("{} override(s)", recurrence.overrides.len()));
    }
    if parts.is_empty() {
        "Custom calendar rule".to_string()
    } else {
        format!("Custom: {}", parts.join(" · "))
    }
}

fn simple_repeat_summary(simple: &SimpleRecurrence, _time_format: TimeFormat) -> String {
    match simple {
        SimpleRecurrence::Daily { interval, .. } => {
            if *interval <= 1 {
                "Daily".to_string()
            } else {
                format!("Every {} days", interval)
            }
        }
        SimpleRecurrence::Weekly {
            interval,
            weekdays: _,
            end: _,
        } => {
            if *interval <= 1 {
                "Weekly".to_string()
            } else {
                format!("Every {} weeks", interval)
            }
        }
        SimpleRecurrence::Monthly { interval, .. } => {
            if *interval <= 1 {
                "Monthly".to_string()
            } else {
                format!("Every {} months", interval)
            }
        }
        SimpleRecurrence::Yearly { interval, .. } => {
            if *interval <= 1 {
                "Yearly".to_string()
            } else {
                format!("Every {} years", interval)
            }
        }
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    out.push_str("...");
    out
}
