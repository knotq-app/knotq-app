use super::*;

pub(super) fn repeat_weekday_labels() -> [RepeatWeekday; 7] {
    [
        RepeatWeekday::Sun,
        RepeatWeekday::Mon,
        RepeatWeekday::Tue,
        RepeatWeekday::Wed,
        RepeatWeekday::Thu,
        RepeatWeekday::Fri,
        RepeatWeekday::Sat,
    ]
}

pub(super) fn default_weekday_for_item(item: &Item) -> RepeatWeekday {
    item.start
        .or(item.end)
        .or(item.available)
        .map(|dt| dt.weekday().num_days_from_monday())
        .map(repeat_weekday_from_index)
        .unwrap_or(RepeatWeekday::Mon)
}

pub(super) fn repeat_weekday_from_index(index: u32) -> RepeatWeekday {
    match index % 7 {
        0 => RepeatWeekday::Mon,
        1 => RepeatWeekday::Tue,
        2 => RepeatWeekday::Wed,
        3 => RepeatWeekday::Thu,
        4 => RepeatWeekday::Fri,
        5 => RepeatWeekday::Sat,
        _ => RepeatWeekday::Sun,
    }
}

pub(super) fn weekday_index(weekday: RepeatWeekday) -> u32 {
    match weekday {
        RepeatWeekday::Mon => 0,
        RepeatWeekday::Tue => 1,
        RepeatWeekday::Wed => 2,
        RepeatWeekday::Thu => 3,
        RepeatWeekday::Fri => 4,
        RepeatWeekday::Sat => 5,
        RepeatWeekday::Sun => 6,
    }
}

pub(super) fn repeat_from_state(state: &RepeatState) -> Recurrence {
    let interval = state.interval.max(1);
    let end = state.end.clone();
    let simple = match state.mode {
        RepeatMode::Daily => SimpleRecurrence::Daily { interval, end },
        RepeatMode::Weekly => {
            let mut weekdays = state.weekdays.clone();
            weekdays.sort_unstable_by_key(|day| weekday_index(*day));
            weekdays.dedup();
            if weekdays.is_empty() {
                weekdays.push(RepeatWeekday::Mon);
            }
            SimpleRecurrence::Weekly {
                interval,
                weekdays,
                end,
            }
        }
        RepeatMode::Monthly => SimpleRecurrence::Monthly { interval, end },
        RepeatMode::Yearly => SimpleRecurrence::Yearly { interval, end },
    };
    CalendarRecurrence {
        rrules: vec![simple_recurrence_rrule(&simple)],
        ..Default::default()
    }
}

pub(super) fn repeat_state_from_recurrence(
    repeat: &Recurrence,
    item: &Item,
) -> Option<RepeatState> {
    if !repeat.rdates.is_empty() || repeat.rrules.len() != 1 {
        return None;
    }
    match parse_simple_rrule(&repeat.rrules[0])? {
        SimpleRecurrence::Daily { interval, end } => Some(RepeatState {
            mode: RepeatMode::Daily,
            interval,
            weekdays: Vec::new(),
            end,
        }),
        SimpleRecurrence::Weekly {
            interval,
            weekdays,
            end,
        } => Some(RepeatState {
            mode: RepeatMode::Weekly,
            interval,
            weekdays: if weekdays.is_empty() {
                vec![default_weekday_for_item(item)]
            } else {
                weekdays
            },
            end,
        }),
        SimpleRecurrence::Monthly { interval, end } => Some(RepeatState {
            mode: RepeatMode::Monthly,
            interval,
            weekdays: Vec::new(),
            end,
        }),
        SimpleRecurrence::Yearly { interval, end } => Some(RepeatState {
            mode: RepeatMode::Yearly,
            interval,
            weekdays: Vec::new(),
            end,
        }),
    }
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

pub(super) fn repeat_weekday_rrule_code(weekday: RepeatWeekday) -> &'static str {
    match weekday {
        RepeatWeekday::Mon => "MO",
        RepeatWeekday::Tue => "TU",
        RepeatWeekday::Wed => "WE",
        RepeatWeekday::Thu => "TH",
        RepeatWeekday::Fri => "FR",
        RepeatWeekday::Sat => "SA",
        RepeatWeekday::Sun => "SU",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_weekday_roundtrip() {
        let item = Item::new("x");
        let state = RepeatState {
            mode: RepeatMode::Weekly,
            interval: 2,
            weekdays: vec![RepeatWeekday::Wed, RepeatWeekday::Mon],
            end: RepeatEnd::Count(10),
        };
        let repeat = repeat_from_state(&state.normalized(&item));
        assert_eq!(
            repeat.rrules,
            vec!["FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10"]
        );
        let parsed = repeat_state_from_recurrence(&repeat, &item).unwrap();
        let (roundtrip, end) = (parsed.weekdays, parsed.end);
        assert!(roundtrip.contains(&RepeatWeekday::Mon));
        assert!(roundtrip.contains(&RepeatWeekday::Wed));
        assert_eq!(end, RepeatEnd::Count(10));
    }

    #[test]
    fn local_date_repeat_end_roundtrips_without_timezone_shift() {
        let item = Item::new("x");
        let date = NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
        let until = local_repeat_until_for_date(date).unwrap();
        let state = RepeatState {
            mode: RepeatMode::Daily,
            interval: 1,
            weekdays: Vec::new(),
            end: RepeatEnd::Until(until),
        };

        let repeat = repeat_from_state(&state.normalized(&item));
        assert_eq!(
            repeat.rrules,
            vec![format!(
                "FREQ=DAILY;INTERVAL=1;UNTIL={}",
                until.format("%Y%m%dT%H%M%SZ")
            )]
        );
        let parsed = repeat_state_from_recurrence(&repeat, &item).unwrap();
        assert_eq!(parsed.end, RepeatEnd::Until(until));
        assert_eq!(
            match parsed.end {
                RepeatEnd::Until(until) => until.with_timezone(&Local).date_naive(),
                _ => unreachable!(),
            },
            date
        );
    }
}
