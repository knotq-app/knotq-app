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
    .unwrap_or_else(|| knotq_l10n::t("event.value.none").to_string())
}

fn format_relative_datetime(time_format: TimeFormat, dt: DateTime<Local>) -> String {
    let today = Local::now().date_naive();
    let date = dt.date_naive();
    let day = if date == today {
        knotq_l10n::t("event.date.today").to_string()
    } else if date == today + Duration::days(1) {
        knotq_l10n::t("event.date.tomorrow").to_string()
    } else if date == today - Duration::days(1) {
        knotq_l10n::t("event.date.yesterday").to_string()
    } else if date > today && date < today + Duration::days(7) {
        knotq_date_util::weekday_short_name(dt.weekday()).to_string()
    } else if date.year() == today.year() {
        format!(
            "{} {:02}",
            knotq_date_util::month_short_name(dt.month()),
            dt.day()
        )
    } else {
        format!(
            "{} {} {:02}",
            dt.year(),
            knotq_date_util::month_short_name(dt.month()),
            dt.day()
        )
    };
    format!("{day} {}", format_time(time_format, dt))
}

pub(super) fn format_lead_time(
    time_format: TimeFormat,
    offset_secs: i64,
    trigger_at: Option<DateTime<Utc>>,
) -> String {
    if offset_secs == 0 {
        return knotq_l10n::t("event.notification.at_time").to_string();
    }
    if offset_secs < 0 {
        if let Some(trigger_at) = trigger_at {
            let fire_at = trigger_at - Duration::seconds(offset_secs);
            return format_relative_datetime(time_format, fire_at.with_timezone(&Local));
        }
    }
    let suffix = if offset_secs > 0 {
        knotq_l10n::t("event.notification.before")
    } else {
        knotq_l10n::t("event.notification.after")
    };
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
        return knotq_l10n::t_count("event.duration.days", days);
    }
    let hours = seconds / 3_600;
    if hours > 0 && seconds % 3_600 == 0 {
        return knotq_l10n::t_count("event.duration.hours", hours);
    }
    let minutes = seconds / 60;
    if minutes > 0 {
        return knotq_l10n::t_count("event.duration.minutes", minutes);
    }
    knotq_l10n::t_count("event.duration.seconds", seconds)
}

pub(super) fn repeat_summary(recurrence: Option<&Recurrence>, time_format: TimeFormat) -> String {
    let Some(recurrence) = recurrence else {
        return knotq_l10n::t("event.value.none").to_string();
    };
    if let Some(simple) = editable_simple_recurrence(recurrence) {
        return simple_repeat_summary(&simple, time_format);
    }
    let mut parts = Vec::new();
    if !recurrence.rrules.is_empty() {
        parts.push(knotq_l10n::t_with(
            "event.repeat.rrule_summary",
            &[("rule", &truncate(&recurrence.rrules.join(", "), 72))],
        ));
    }
    if !recurrence.rdates.is_empty() {
        parts.push(knotq_l10n::t_count(
            "event.repeat.extra_dates",
            recurrence.rdates.len() as i64,
        ));
    }
    if !recurrence.exdates.is_empty() {
        parts.push(knotq_l10n::t_count(
            "event.repeat.exception_dates",
            recurrence.exdates.len() as i64,
        ));
    }
    if !recurrence.overrides.is_empty() {
        parts.push(knotq_l10n::t_count(
            "event.repeat.overrides",
            recurrence.overrides.len() as i64,
        ));
    }
    if parts.is_empty() {
        knotq_l10n::t("event.repeat.custom_rule").to_string()
    } else {
        knotq_l10n::t_with("event.repeat.custom_summary", &[("parts", &parts.join(" · "))])
    }
}

fn simple_repeat_summary(simple: &SimpleRecurrence, _time_format: TimeFormat) -> String {
    match simple {
        SimpleRecurrence::Daily { interval, .. } => {
            if *interval <= 1 {
                knotq_l10n::t("repeat.daily").to_string()
            } else {
                knotq_l10n::t_count("repeat.every_n_days", *interval as i64)
            }
        }
        SimpleRecurrence::Weekly {
            interval,
            weekdays: _,
            end: _,
        } => {
            if *interval <= 1 {
                knotq_l10n::t("repeat.weekly").to_string()
            } else {
                knotq_l10n::t_count("repeat.every_n_weeks", *interval as i64)
            }
        }
        SimpleRecurrence::Monthly { interval, .. } => {
            if *interval <= 1 {
                knotq_l10n::t("repeat.monthly").to_string()
            } else {
                knotq_l10n::t_count("repeat.every_n_months", *interval as i64)
            }
        }
        SimpleRecurrence::Yearly { interval, .. } => {
            if *interval <= 1 {
                knotq_l10n::t("repeat.yearly").to_string()
            } else {
                knotq_l10n::t_count("repeat.every_n_years", *interval as i64)
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
