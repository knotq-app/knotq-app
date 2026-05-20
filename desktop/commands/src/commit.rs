use crate::{Command, DateKind};
use chrono::{DateTime, Utc};
use knotq_model::{CalendarRecurrence, Item, ItemId, OccurrenceId, SchemeId};
use knotq_rrule::{scoped_date_edit_recurrence, RecurrenceEditScope};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DateEditScope {
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
                draft.occurrence.clone(),
                draft.occurrence_index,
                draft.start_dirty,
                draft.draft_start,
                draft.end_dirty,
                draft.draft_end,
                scope,
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
