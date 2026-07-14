use crate::{Command, DateKind};
use chrono::{DateTime, Duration, Utc};
use knotq_model::{
    CalendarRecurrence, Item, ItemId, ItemKind, ItemMarker, OccurrenceId, Recurrence, RepeatEnd,
    SchemeId, SimpleRecurrence,
};
use knotq_rrule::ical::{parse_rrule_until, parse_rrule_weekdays};
use knotq_rrule::{scoped_date_edit_recurrence, RecurrenceEditScope, ScopedDateEdit};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DateEditScope {
    ThisEvent,
    AllFuture,
    AllEvents,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventDeleteScope {
    ThisEvent,
    AllFuture,
    AllEvents,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventPopupDraft {
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub occurrence: OccurrenceId,
    pub occurrence_index: usize,
    pub draft_start: Option<DateTime<Utc>>,
    pub draft_end: Option<DateTime<Utc>>,
    pub draft_repeats: Option<CalendarRecurrence>,
    pub draft_notification_offset_secs: Option<i64>,
    pub draft_done: bool,
    pub start_dirty: bool,
    pub end_dirty: bool,
    pub repeats_dirty: bool,
    pub notification_dirty: bool,
    pub done_dirty: bool,
}

pub fn event_popup_commit_commands(
    item: &Item,
    draft: &EventPopupDraft,
    scope: DateEditScope,
) -> Vec<Command> {
    let mut commands = Vec::new();
    let scheme = draft.scheme_id;
    let item_id = draft.item_id;
    let occurrence = draft.occurrence.clone();

    let promote_occurrence_to_single = draft.repeats_dirty
        && !draft.occurrence.is_single()
        && item.repeats.is_some()
        && draft.draft_repeats.is_none();
    let start_changed =
        (draft.start_dirty || promote_occurrence_to_single) && item.start != draft.draft_start;
    let end_changed =
        (draft.end_dirty || promote_occurrence_to_single) && item.end != draft.draft_end;
    let scoped_date_change = !promote_occurrence_to_single
        && !draft.occurrence.is_single()
        && item.repeats.is_some()
        && (start_changed || end_changed);

    if scoped_date_change {
        let scope = match scope {
            DateEditScope::ThisEvent => RecurrenceEditScope::ThisEvent,
            DateEditScope::AllFuture => RecurrenceEditScope::AllFuture,
            DateEditScope::AllEvents => RecurrenceEditScope::AllEvents,
        };
        if let Some(recurrence) = draft.draft_repeats.as_ref().or(item.repeats.as_ref()) {
            if let Some(repeats) = scoped_date_edit_recurrence(
                item,
                recurrence,
                ScopedDateEdit {
                    occurrence: draft.occurrence.clone(),
                    occurrence_index: draft.occurrence_index,
                    start_dirty: draft.start_dirty,
                    draft_start: draft.draft_start,
                    end_dirty: draft.end_dirty,
                    draft_end: draft.draft_end,
                    scope,
                },
            ) {
                commands.push(Command::SetItemRecurrence {
                    scheme,
                    item: item_id,
                    repeats: Some(repeats),
                });
            }
        }
    }

    if start_changed && (!scoped_date_change || scope != DateEditScope::ThisEvent) {
        push_date_command(
            &mut commands,
            scheme,
            item_id,
            DateKind::Start,
            draft.draft_start,
        );
    }
    if end_changed && (!scoped_date_change || scope != DateEditScope::ThisEvent) {
        push_date_command(
            &mut commands,
            scheme,
            item_id,
            DateKind::End,
            draft.draft_end,
        );
    }
    if (!scoped_date_change || scope == DateEditScope::AllEvents)
        && draft.repeats_dirty
        && item.repeats != draft.draft_repeats
    {
        commands.push(Command::SetItemRecurrence {
            scheme,
            item: item_id,
            repeats: draft.draft_repeats.clone(),
        });
    }
    if draft.notification_dirty {
        commands.push(Command::SetOccurrenceNotificationOffset {
            scheme,
            item: item_id,
            occurrence: occurrence.clone(),
            offset_secs: draft.draft_notification_offset_secs,
        });
    } else if start_changed || end_changed {
        if let Some(command) = reset_after_trigger_notification_to_default_command(
            item,
            scheme,
            item_id,
            occurrence.clone(),
            draft.draft_start,
            draft.draft_end,
            Utc::now(),
        ) {
            commands.push(command);
        }
    }
    if draft.done_dirty {
        let state = item.state_for_occurrence(&occurrence);
        if state.is_done() != draft.draft_done {
            commands.push(Command::ToggleOccurrence {
                scheme,
                item: item_id,
                occurrence,
            });
        }
    }
    commands
}

pub fn reset_after_trigger_notification_to_default_command(
    item: &Item,
    scheme: SchemeId,
    item_id: ItemId,
    occurrence: OccurrenceId,
    draft_start: Option<DateTime<Utc>>,
    draft_end: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<Command> {
    let offset_secs = item
        .state_for_occurrence(&occurrence)
        .notification_offset_secs?;
    if offset_secs >= 0 {
        return None;
    }
    let trigger_at = notification_trigger_at(item.marker, draft_start, draft_end)?;
    if trigger_at <= now {
        return None;
    }
    Some(Command::SetOccurrenceNotificationOffset {
        scheme,
        item: item_id,
        occurrence,
        offset_secs: None,
    })
}

fn notification_trigger_at(
    marker: ItemMarker,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match item_kind_for_dates(marker, start, end) {
        ItemKind::Reminder | ItemKind::Event => start,
        ItemKind::Assignment => end,
        ItemKind::Procedure => None,
    }
}

fn item_kind_for_dates(
    marker: ItemMarker,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> ItemKind {
    if marker != ItemMarker::Checkbox {
        return ItemKind::Procedure;
    }
    match (start.is_some(), end.is_some()) {
        (true, true) => ItemKind::Event,
        (true, false) => ItemKind::Reminder,
        (false, true) => ItemKind::Assignment,
        (false, false) => ItemKind::Procedure,
    }
}

fn push_date_command(
    commands: &mut Vec<Command>,
    scheme: SchemeId,
    item_id: ItemId,
    kind: DateKind,
    date: Option<DateTime<Utc>>,
) {
    commands.push(Command::SetItemDate {
        scheme,
        item: item_id,
        kind,
        date,
    });
}

pub fn event_popup_delete_command(
    item: &Item,
    scheme: SchemeId,
    item_id: ItemId,
    occurrence: OccurrenceId,
    occurrence_index: usize,
    scope: EventDeleteScope,
) -> Option<Command> {
    match scope {
        EventDeleteScope::ThisEvent if !occurrence.is_single() && item.repeats.is_some() => {
            let repeats = item.repeats.as_ref()?;
            let next_repeats = recurrence_without_occurrence(repeats, &occurrence)?;
            Some(Command::SetItemRecurrence {
                scheme,
                item: item_id,
                repeats: Some(next_repeats),
            })
        }
        EventDeleteScope::AllFuture if !occurrence.is_single() && item.repeats.is_some() => {
            let repeats = item.repeats.as_ref()?;
            match recurrence_without_this_and_future(repeats, &occurrence, occurrence_index)? {
                Some(next_repeats) => Some(Command::SetItemRecurrence {
                    scheme,
                    item: item_id,
                    repeats: Some(next_repeats),
                }),
                None => Some(Command::DeleteItem {
                    scheme,
                    item: item_id,
                }),
            }
        }
        EventDeleteScope::ThisEvent | EventDeleteScope::AllFuture | EventDeleteScope::AllEvents => {
            Some(Command::DeleteItem {
                scheme,
                item: item_id,
            })
        }
    }
}

pub fn recurrence_can_delete_future(repeat: &Recurrence) -> bool {
    editable_simple_recurrence(repeat).is_some()
}

pub fn recurrence_without_this_and_future(
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

pub fn recurrence_without_occurrence(
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

fn editable_simple_recurrence(repeat: &Recurrence) -> Option<SimpleRecurrence> {
    if !repeat.rdates.is_empty() || repeat.rrules.len() != 1 {
        return None;
    }
    parse_simple_rrule(&repeat.rrules[0])
}

fn recurrence_with_simple(previous: Option<&Recurrence>, simple: SimpleRecurrence) -> Recurrence {
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

fn parse_simple_rrule(raw_rule: &str) -> Option<SimpleRecurrence> {
    let fields = parse_rrule_fields(raw_rule);
    let interval = fields
        .get("INTERVAL")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let end = fields
        .get("COUNT")
        .and_then(|value| value.parse::<usize>().ok())
        .map(RepeatEnd::Count)
        .or_else(|| {
            fields
                .get("UNTIL")
                .and_then(|value| parse_rrule_until(value))
                .map(RepeatEnd::Until)
        })
        .unwrap_or(RepeatEnd::Never);
    let freq = fields.get("FREQ").map(String::as_str)?;
    let weekdays = fields
        .get("BYDAY")
        .map(|value| parse_rrule_weekdays(value))
        .unwrap_or_default();

    for key in fields.keys() {
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

fn parse_rrule_fields(raw_rule: &str) -> std::collections::BTreeMap<String, String> {
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

fn simple_recurrence_rrule(simple: &SimpleRecurrence) -> String {
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
                        .map(|day| knotq_rrule::weekday_util::repeat_weekday_rrule_code(*day))
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
        RepeatEnd::Count(count) => parts.push(format!("COUNT={count}")),
        RepeatEnd::Until(until) => {
            parts.push(format!("UNTIL={}", until.format("%Y%m%dT%H%M%SZ")));
        }
    }
    parts.join(";")
}
