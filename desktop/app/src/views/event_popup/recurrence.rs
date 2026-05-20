use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EventRepeatMode {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl EventRepeatMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Yearly => "Yearly",
        }
    }
}

pub(super) fn event_repeat_mode(repeat: &Recurrence) -> Option<EventRepeatMode> {
    match editable_simple_recurrence(repeat)? {
        SimpleRecurrence::Daily { .. } => Some(EventRepeatMode::Daily),
        SimpleRecurrence::Weekly { .. } => Some(EventRepeatMode::Weekly),
        SimpleRecurrence::Monthly { .. } => Some(EventRepeatMode::Monthly),
        SimpleRecurrence::Yearly { .. } => Some(EventRepeatMode::Yearly),
    }
}

pub(super) fn simple_repeat_end(repeat: &Recurrence) -> Option<RepeatEnd> {
    match editable_simple_recurrence(repeat)? {
        SimpleRecurrence::Daily { end, .. }
        | SimpleRecurrence::Weekly { end, .. }
        | SimpleRecurrence::Monthly { end, .. }
        | SimpleRecurrence::Yearly { end, .. } => Some(end),
    }
}

pub(super) fn event_repeat_for_mode(item: &Item, mode: EventRepeatMode) -> Recurrence {
    let existing = item.repeats.as_ref().and_then(editable_simple_recurrence);
    let (interval, end) = match existing.as_ref() {
        Some(
            SimpleRecurrence::Daily { interval, end }
            | SimpleRecurrence::Weekly { interval, end, .. }
            | SimpleRecurrence::Monthly { interval, end }
            | SimpleRecurrence::Yearly { interval, end },
        ) => ((*interval).max(1), end.clone()),
        None => (1, RepeatEnd::Never),
    };
    let simple = match mode {
        EventRepeatMode::Daily => SimpleRecurrence::Daily { interval, end },
        EventRepeatMode::Weekly => {
            let weekdays = match existing.as_ref() {
                Some(SimpleRecurrence::Weekly { weekdays, .. }) if !weekdays.is_empty() => {
                    weekdays.clone()
                }
                _ => vec![default_repeat_weekday(item)],
            };
            SimpleRecurrence::Weekly {
                interval,
                weekdays,
                end,
            }
        }
        EventRepeatMode::Monthly => SimpleRecurrence::Monthly { interval, end },
        EventRepeatMode::Yearly => SimpleRecurrence::Yearly { interval, end },
    };
    recurrence_with_simple(item.repeats.as_ref(), simple)
}

pub(super) fn repeat_with_end(repeat: &Recurrence, next_end: RepeatEnd) -> Option<Recurrence> {
    let simple = match editable_simple_recurrence(repeat)? {
        SimpleRecurrence::Daily { interval, .. } => SimpleRecurrence::Daily {
            interval,
            end: next_end,
        },
        SimpleRecurrence::Weekly {
            interval, weekdays, ..
        } => SimpleRecurrence::Weekly {
            interval,
            weekdays,
            end: next_end,
        },
        SimpleRecurrence::Monthly { interval, .. } => SimpleRecurrence::Monthly {
            interval,
            end: next_end,
        },
        SimpleRecurrence::Yearly { interval, .. } => SimpleRecurrence::Yearly {
            interval,
            end: next_end,
        },
    };
    Some(recurrence_with_simple(Some(repeat), simple))
}

pub(super) fn editable_simple_recurrence(repeat: &Recurrence) -> Option<SimpleRecurrence> {
    if !repeat.rdates.is_empty() || repeat.rrules.len() != 1 {
        return None;
    }
    parse_simple_rrule(&repeat.rrules[0])
}

pub(super) fn recurrence_with_simple(
    previous: Option<&Recurrence>,
    simple: SimpleRecurrence,
) -> Recurrence {
    if let Some(previous) = previous {
        if editable_simple_recurrence(previous).is_some() {
            let mut next = previous.clone();
            next.rrules = vec![simple_recurrence_rrule(&simple)];
            next.raw_import = None;
            return next;
        }
    }
    CalendarRecurrence {
        rrules: vec![simple_recurrence_rrule(&simple)],
        ..Default::default()
    }
}

pub(super) fn parse_simple_rrule(raw_rule: &str) -> Option<SimpleRecurrence> {
    let fields = parse_rrule_fields(raw_rule)?;
    let interval = fields
        .iter()
        .find(|(key, _)| *key == "INTERVAL")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let end = fields
        .iter()
        .find(|(key, _)| *key == "COUNT")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .map(RepeatEnd::Count)
        .or_else(|| {
            fields
                .iter()
                .find(|(key, _)| *key == "UNTIL")
                .and_then(|(_, value)| parse_rrule_until(value))
                .map(RepeatEnd::Until)
        })
        .unwrap_or(RepeatEnd::Never);
    let freq = fields
        .iter()
        .find(|(key, _)| *key == "FREQ")
        .map(|(_, value)| value.as_str())?;
    let byday = fields
        .iter()
        .find(|(key, _)| *key == "BYDAY")
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
        "DAILY" if byday.is_empty() => Some(SimpleRecurrence::Daily { interval, end }),
        "WEEKLY" => Some(SimpleRecurrence::Weekly {
            interval,
            weekdays: byday,
            end,
        }),
        "MONTHLY" if byday.is_empty() => Some(SimpleRecurrence::Monthly { interval, end }),
        "YEARLY" if byday.is_empty() => Some(SimpleRecurrence::Yearly { interval, end }),
        _ => None,
    }
}

pub(super) fn parse_rrule_fields(raw_rule: &str) -> Option<Vec<(String, String)>> {
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
    (!fields.is_empty()).then_some(fields)
}

pub(super) fn parse_rrule_until(value: &str) -> Option<DateTime<Utc>> {
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
                .and_then(local_repeat_until_for_date)
        })
}

pub(super) fn local_repeat_until_for_date(date: NaiveDate) -> Option<DateTime<Utc>> {
    let local_end = date.and_hms_opt(23, 59, 59)?;
    Local
        .from_local_datetime(&local_end)
        .latest()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(super) fn repeat_end_for_local_date(date: NaiveDate) -> RepeatEnd {
    RepeatEnd::Until(
        local_repeat_until_for_date(date)
            .expect("23:59:59 should resolve for a valid local calendar date"),
    )
}

pub(super) fn parse_rrule_weekdays(value: &str) -> Vec<RepeatWeekday> {
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

pub(super) fn recurrence_can_delete_future(repeat: &Recurrence) -> bool {
    editable_simple_recurrence(repeat).is_some()
}

pub(super) fn recurrence_without_this_and_future(
    repeat: &Recurrence,
    occurrence: &OccurrenceId,
    occurrence_index: usize,
) -> Option<Option<Recurrence>> {
    if occurrence_index == 0 {
        return Some(None);
    }
    let OccurrenceId::Recurring { original_start } = occurrence else {
        return None;
    };
    let until = RepeatEnd::Until(original_start.as_utc_lossy() - Duration::seconds(1));
    let simple = match editable_simple_recurrence(repeat)? {
        SimpleRecurrence::Daily { interval, .. } => SimpleRecurrence::Daily {
            interval,
            end: until.clone(),
        },
        SimpleRecurrence::Weekly {
            interval, weekdays, ..
        } => SimpleRecurrence::Weekly {
            interval,
            weekdays,
            end: until.clone(),
        },
        SimpleRecurrence::Monthly { interval, .. } => SimpleRecurrence::Monthly {
            interval,
            end: until.clone(),
        },
        SimpleRecurrence::Yearly { interval, .. } => SimpleRecurrence::Yearly {
            interval,
            end: until,
        },
    };
    Some(Some(recurrence_with_simple(Some(repeat), simple)))
}

pub(super) fn recurrence_without_occurrence(
    repeat: &Recurrence,
    occurrence: &OccurrenceId,
) -> Option<Recurrence> {
    let OccurrenceId::Recurring { original_start } = occurrence else {
        return None;
    };
    let mut complex = repeat.clone();
    let deleted_anchor = original_start.as_utc_lossy();
    if !complex
        .exdates
        .iter()
        .any(|date| date.as_utc_lossy() == deleted_anchor)
    {
        complex.exdates.push(original_start.clone());
    }
    Some(complex)
}

pub(super) fn simple_recurrence_rrule(simple: &SimpleRecurrence) -> String {
    let mut parts = match simple {
        SimpleRecurrence::Daily { interval, .. } => {
            vec![
                "FREQ=DAILY".to_string(),
                format!("INTERVAL={}", (*interval).max(1)),
            ]
        }
        SimpleRecurrence::Weekly {
            interval, weekdays, ..
        } => {
            let mut parts = vec![
                "FREQ=WEEKLY".to_string(),
                format!("INTERVAL={}", (*interval).max(1)),
            ];
            if !weekdays.is_empty() {
                parts.push(format!(
                    "BYDAY={}",
                    weekdays
                        .iter()
                        .map(|day| repeat_weekday_rrule_code(*day))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            parts
        }
        SimpleRecurrence::Monthly { interval, .. } => {
            vec![
                "FREQ=MONTHLY".to_string(),
                format!("INTERVAL={}", (*interval).max(1)),
            ]
        }
        SimpleRecurrence::Yearly { interval, .. } => {
            vec![
                "FREQ=YEARLY".to_string(),
                format!("INTERVAL={}", (*interval).max(1)),
            ]
        }
    };

    match simple.repeat_end() {
        RepeatEnd::Never => {}
        RepeatEnd::Count(count) => parts.push(format!("COUNT={}", count)),
        RepeatEnd::Until(until) => {
            parts.push(format!("UNTIL={}", format_rrule_until(until)));
        }
    }
    parts.join(";")
}

pub(super) fn format_rrule_until(until: &DateTime<Utc>) -> String {
    until.format("%Y%m%dT%H%M%SZ").to_string()
}
