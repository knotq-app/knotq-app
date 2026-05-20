mod event;
mod map;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use knotq_model::CalendarRecurrence;
use knotq_rrule::ical::{format_rrule, parse_rrule_string};

pub use event::ImportedEvent;
pub use map::{event_update_commands, map_to_commands};

pub fn parse_ical(bytes: &[u8]) -> Vec<ImportedEvent> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    unfold_lines(text)
        .split("BEGIN:VEVENT")
        .skip(1)
        .filter_map(parse_event_section)
        .collect()
}

fn parse_event_section(section: &str) -> Option<ImportedEvent> {
    let summary = property_value(section, "SUMMARY").unwrap_or("").trim();
    (!summary.is_empty()).then(|| {
        let uid = property_value(section, "UID").unwrap_or(summary).trim();
        ImportedEvent {
            uid: uid.to_string(),
            summary: summary.to_string(),
            start: property_value(section, "DTSTART").and_then(parse_ical_datetime),
            end: property_value(section, "DTEND").and_then(parse_ical_datetime),
            recurrence: property_value(section, "RRULE").and_then(parse_recurrence),
        }
    })
}

fn unfold_lines(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            out.push_str(line.trim_start());
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    out
}

fn property_value<'a>(section: &'a str, name: &str) -> Option<&'a str> {
    section.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        let property = key.split(';').next()?.trim();
        property.eq_ignore_ascii_case(name).then_some(value)
    })
}

fn parse_ical_datetime(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
        .ok()
        .map(|dt| DateTime::from_naive_utc_and_offset(dt, Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
                .ok()
                .map(|dt| DateTime::from_naive_utc_and_offset(dt, Utc))
        })
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y%m%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .map(|dt| DateTime::from_naive_utc_and_offset(dt, Utc))
        })
}

fn parse_recurrence(value: &str) -> Option<CalendarRecurrence> {
    let fields = parse_rrule_string(value);
    (!fields.is_empty()).then(|| CalendarRecurrence {
        rrules: vec![format_rrule(&fields)],
        ..CalendarRecurrence::default()
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn parse_ical_extracts_event_dates_and_rrule() {
        let bytes = br#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:class-1
SUMMARY:MATH 15
DTSTART:20260105T113000Z
DTEND:20260105T123000Z
RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR
END:VEVENT
END:VCALENDAR"#;

        let events = parse_ical(bytes);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].uid, "class-1");
        assert_eq!(events[0].summary, "MATH 15");
        assert_eq!(
            events[0].start,
            Some(Utc.with_ymd_and_hms(2026, 1, 5, 11, 30, 0).unwrap())
        );
        assert_eq!(
            events[0].end,
            Some(Utc.with_ymd_and_hms(2026, 1, 5, 12, 30, 0).unwrap())
        );
        assert_eq!(
            events[0].recurrence.as_ref().unwrap().rrules,
            vec!["BYDAY=MO,WE,FR;FREQ=WEEKLY"]
        );
    }
}
