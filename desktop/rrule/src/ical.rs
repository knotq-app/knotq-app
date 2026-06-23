use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc, Weekday};
use std::collections::BTreeMap;

use knotq_model::RepeatWeekday;

pub fn parse_rrule_string(raw_rule: &str) -> BTreeMap<String, String> {
    parse_rrule_fields(raw_rule)
}

pub fn format_rrule(fields: &BTreeMap<String, String>) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(";")
}

pub(crate) fn parse_rrule_fields(raw_rule: &str) -> BTreeMap<String, String> {
    raw_rule
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
        .collect()
}

pub fn parse_rrule_weekdays(value: &str) -> Vec<RepeatWeekday> {
    value
        .split(',')
        .filter_map(|part| {
            let day = part
                .trim()
                .trim_start_matches(|ch: char| ch == '+' || ch == '-' || ch.is_ascii_digit());
            match day {
                "MO" => Some(RepeatWeekday::Mon),
                "TU" => Some(RepeatWeekday::Tue),
                "WE" => Some(RepeatWeekday::Wed),
                "TH" => Some(RepeatWeekday::Thu),
                "FR" => Some(RepeatWeekday::Fri),
                "SA" => Some(RepeatWeekday::Sat),
                "SU" => Some(RepeatWeekday::Sun),
                _ => None,
            }
        })
        .collect()
}

pub fn parse_rrule_until(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
                .ok()
                .map(|dt| DateTime::from_naive_utc_and_offset(dt, Utc))
        })
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y%m%d")
                .ok()
                .and_then(knotq_date_util::local_date_repeat_until_utc)
        })
}

pub(crate) fn repeat_weekday_from_chrono(weekday: Weekday) -> RepeatWeekday {
    match weekday {
        Weekday::Mon => RepeatWeekday::Mon,
        Weekday::Tue => RepeatWeekday::Tue,
        Weekday::Wed => RepeatWeekday::Wed,
        Weekday::Thu => RepeatWeekday::Thu,
        Weekday::Fri => RepeatWeekday::Fri,
        Weekday::Sat => RepeatWeekday::Sat,
        Weekday::Sun => RepeatWeekday::Sun,
    }
}
