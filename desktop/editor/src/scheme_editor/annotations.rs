use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use knotq_commands::DateKind;
use knotq_rrule::ical::{parse_rrule_until, parse_rrule_weekdays};
use knotq_model::TimeFormat;
use knotq_model::{
    CalendarDateTime, Item, ItemMarker, OccurrenceId, OccurrenceOverrideStatus, Recurrence,
    RepeatEnd, RepeatWeekday, SimpleRecurrence,
};
use std::collections::BTreeSet;

use knotq_date_util::{format_contextual_date, format_contextual_datetime, format_time};

pub(super) fn annotation_text(item: &Item, time_format: TimeFormat) -> Option<String> {
    let mut text = annotation_parts(item, time_format)
        .map(|parts| {
            parts
                .into_iter()
                .map(|(_, label)| label)
                .collect::<Vec<_>>()
                .join(" \u{2192} ")
        })
        .unwrap_or_default();

    if let Some(repeat) = item.repeats.as_ref() {
        if !text.is_empty() {
            text.push_str(" · ");
        }
        text.push_str(&format_repeat_annotation(repeat));
    }

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(super) fn annotation_parts(
    item: &Item,
    time_format: TimeFormat,
) -> Option<Vec<(DateKind, String)>> {
    if item.marker != ItemMarker::Checkbox {
        return None;
    }
    match (item.start, item.end) {
        (Some(start), Some(end)) => Some(vec![
            (
                DateKind::Start,
                format_annotation_datetime(start, None, time_format),
            ),
            (
                DateKind::End,
                format_annotation_datetime(end, Some(start), time_format),
            ),
        ]),
        (Some(start), None) => Some(vec![(
            DateKind::Start,
            knotq_l10n::t_with(
                "editor.annotation.at",
                &[(
                    "date",
                    &format_annotation_datetime(start, None, time_format),
                )],
            ),
        )]),
        (None, Some(end)) => Some(vec![(
            DateKind::End,
            knotq_l10n::t_with(
                "editor.annotation.due",
                &[("date", &format_annotation_datetime(end, None, time_format))],
            ),
        )]),
        (None, None) => None,
    }
}

pub(super) fn format_repeat_annotation(repeat: &Recurrence) -> String {
    format_repeat_annotation_for_year(repeat, Local::now().year())
}

pub(super) fn format_repeat_annotation_for_year(
    repeat: &Recurrence,
    reference_year: i32,
) -> String {
    let mut text = if let Some(simple) = editable_simple_recurrence(repeat) {
        format_simple_repeat_annotation(&simple, reference_year)
    } else {
        knotq_l10n::t("editor.repeat.complex").to_string()
    };
    text.push_str(&repeat_exception_suffix(repeat, reference_year));
    text
}

fn editable_simple_recurrence(repeat: &Recurrence) -> Option<SimpleRecurrence> {
    if !repeat.rdates.is_empty() || repeat.rrules.len() != 1 {
        return None;
    }
    parse_simple_rrule(&repeat.rrules[0])
}

fn parse_simple_rrule(raw_rule: &str) -> Option<SimpleRecurrence> {
    let fields = raw_rule
        .trim()
        .trim_start_matches("RRULE:")
        .split(';')
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((
                key.trim().to_ascii_uppercase(),
                value.trim().to_ascii_uppercase(),
            ))
        })
        .collect::<Vec<_>>();
    if fields.is_empty() {
        return None;
    }
    let interval = fields
        .iter()
        .find(|(key, _)| key == "INTERVAL")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let end = fields
        .iter()
        .find(|(key, _)| key == "COUNT")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .map(RepeatEnd::Count)
        .or_else(|| {
            fields
                .iter()
                .find(|(key, _)| key == "UNTIL")
                .and_then(|(_, value)| parse_rrule_until(value))
                .map(RepeatEnd::Until)
        })
        .unwrap_or(RepeatEnd::Never);
    let freq = fields
        .iter()
        .find(|(key, _)| key == "FREQ")
        .map(|(_, value)| value.as_str())?;
    let weekdays = fields
        .iter()
        .find(|(key, _)| key == "BYDAY")
        .map(|(_, value)| parse_rrule_weekdays(value))
        .unwrap_or_default();

    for (key, _) in &fields {
        if !matches!(
            key.as_str(),
            "FREQ" | "INTERVAL" | "COUNT" | "UNTIL" | "BYDAY" | "WKST"
        ) {
            return None;
        }
    }

    match freq {
        "DAILY" if weekdays.is_empty() => Some(SimpleRecurrence::Daily { interval, end }),
        "WEEKLY" => Some(SimpleRecurrence::Weekly {
            interval,
            weekdays,
            end,
        }),
        "MONTHLY" if weekdays.is_empty() => Some(SimpleRecurrence::Monthly { interval, end }),
        "YEARLY" if weekdays.is_empty() => Some(SimpleRecurrence::Yearly { interval, end }),
        _ => None,
    }
}

fn format_simple_repeat_annotation(repeat: &SimpleRecurrence, reference_year: i32) -> String {
    let suffix = repeat_end_suffix(repeat.repeat_end(), reference_year);
    match repeat {
        SimpleRecurrence::Daily { interval, .. } => {
            if *interval <= 1 {
                format!("{}{suffix}", knotq_l10n::t("editor.repeat.daily"))
            } else {
                format!(
                    "{}{suffix}",
                    knotq_l10n::t_with("editor.repeat.every_days", &[("count", &interval.to_string())])
                )
            }
        }
        SimpleRecurrence::Weekly {
            interval, weekdays, ..
        } => {
            let days = format_weekdays(weekdays);
            if *interval <= 1 {
                format!("{}{days}{suffix}", knotq_l10n::t("editor.repeat.weekly"))
            } else {
                format!(
                    "{}{days}{suffix}",
                    knotq_l10n::t_with("editor.repeat.every_weeks", &[("count", &interval.to_string())])
                )
            }
        }
        SimpleRecurrence::Monthly { interval, .. } => {
            if *interval <= 1 {
                format!("{}{suffix}", knotq_l10n::t("editor.repeat.monthly"))
            } else {
                format!(
                    "{}{suffix}",
                    knotq_l10n::t_with("editor.repeat.every_months", &[("count", &interval.to_string())])
                )
            }
        }
        SimpleRecurrence::Yearly { interval, .. } => {
            if *interval <= 1 {
                format!("{}{suffix}", knotq_l10n::t("editor.repeat.yearly"))
            } else {
                format!(
                    "{}{suffix}",
                    knotq_l10n::t_with("editor.repeat.every_years", &[("count", &interval.to_string())])
                )
            }
        }
    }
}

fn repeat_end_suffix(end: &RepeatEnd, reference_year: i32) -> String {
    match end {
        RepeatEnd::Never => String::new(),
        RepeatEnd::Count(count) => {
            knotq_l10n::t_with("editor.repeat.times_suffix", &[("count", &count.to_string())])
        }
        RepeatEnd::Until(until) => {
            let date = until.with_timezone(&Local).date_naive();
            knotq_l10n::t_with(
                "editor.repeat.until_suffix",
                &[("date", &format_contextual_date(date, reference_year))],
            )
        }
    }
}

fn repeat_exception_suffix(repeat: &Recurrence, reference_year: i32) -> String {
    let special_dates = repeat_special_dates(repeat);
    let skip_dates = repeat_skip_dates(repeat, &special_dates);
    let mut parts = Vec::new();

    if !skip_dates.is_empty() {
        parts.push(knotq_l10n::t_with(
            "editor.repeat.skip_suffix",
            &[("dates", &format_contextual_dates(skip_dates, reference_year))],
        ));
    }
    if !special_dates.is_empty() {
        parts.push(knotq_l10n::t_with(
            "editor.repeat.special_suffix",
            &[("dates", &format_contextual_dates(special_dates, reference_year))],
        ));
    }

    if parts.is_empty() {
        String::new()
    } else {
        knotq_l10n::t_with("editor.repeat.parts_suffix", &[("parts", &parts.join("; "))])
    }
}

fn repeat_special_dates(repeat: &Recurrence) -> BTreeSet<NaiveDate> {
    let mut dates = repeat
        .rdates
        .iter()
        .map(calendar_date_time_display_date)
        .collect::<BTreeSet<_>>();

    dates.extend(repeat.overrides.iter().filter_map(|override_| {
        (override_.status == OccurrenceOverrideStatus::Active)
            .then(|| occurrence_display_date(&override_.occurrence))
            .flatten()
    }));

    dates
}

fn repeat_skip_dates(
    repeat: &Recurrence,
    special_dates: &BTreeSet<NaiveDate>,
) -> BTreeSet<NaiveDate> {
    let mut dates = repeat
        .exdates
        .iter()
        .map(calendar_date_time_display_date)
        .collect::<BTreeSet<_>>();

    dates.extend(repeat.overrides.iter().filter_map(|override_| {
        (override_.status == OccurrenceOverrideStatus::Cancelled)
            .then(|| occurrence_display_date(&override_.occurrence))
            .flatten()
    }));

    dates
        .into_iter()
        .filter(|date| !special_dates.contains(date))
        .collect()
}

fn format_contextual_dates(dates: BTreeSet<NaiveDate>, reference_year: i32) -> String {
    dates
        .into_iter()
        .map(|date| format_contextual_date(date, reference_year))
        .collect::<Vec<_>>()
        .join(", ")
}

fn occurrence_display_date(occurrence: &OccurrenceId) -> Option<NaiveDate> {
    match occurrence {
        OccurrenceId::Single => None,
        OccurrenceId::Recurring { original_start } => {
            Some(calendar_date_time_display_date(original_start))
        }
    }
}

fn calendar_date_time_display_date(value: &CalendarDateTime) -> NaiveDate {
    match value {
        CalendarDateTime::Date { date } => *date,
        CalendarDateTime::DateTimeUtc { datetime } => datetime.with_timezone(&Local).date_naive(),
        CalendarDateTime::DateTimeWithZone { local, .. } => local.date(),
    }
}

fn format_weekdays(weekdays: &[RepeatWeekday]) -> String {
    if weekdays.is_empty() {
        return String::new();
    }
    let days = weekdays
        .iter()
        .map(|weekday| match weekday {
            RepeatWeekday::Mon => knotq_l10n::t("editor.weekday.mon"),
            RepeatWeekday::Tue => knotq_l10n::t("editor.weekday.tue"),
            RepeatWeekday::Wed => knotq_l10n::t("editor.weekday.wed"),
            RepeatWeekday::Thu => knotq_l10n::t("editor.weekday.thu"),
            RepeatWeekday::Fri => knotq_l10n::t("editor.weekday.fri"),
            RepeatWeekday::Sat => knotq_l10n::t("editor.weekday.sat"),
            RepeatWeekday::Sun => knotq_l10n::t("editor.weekday.sun"),
        })
        .collect::<Vec<_>>()
        .join(",");
    knotq_l10n::t_with("editor.repeat.weekdays_suffix", &[("days", &days)])
}

fn format_annotation_datetime(
    dt: DateTime<Utc>,
    previous: Option<DateTime<Utc>>,
    time_format: TimeFormat,
) -> String {
    let local = dt.with_timezone(&Local);
    if previous
        .map(|previous| previous.with_timezone(&Local).date_naive() == local.date_naive())
        .unwrap_or(false)
    {
        return format_time(time_format, local);
    }

    let today = Local::now().date_naive();
    if local.date_naive() == today {
        format_time(time_format, local)
    } else {
        format_contextual_datetime(time_format, local, today.year())
    }
}
