use chrono::{DateTime, Duration, Utc};
use knotq_date_util::DateRange;
use knotq_model::{Item, Occurrence, SchemeId, Workspace};
use knotq_rrule::{DefaultExpander, OccurrenceExpander};

use crate::format::{body_for, title_for};
use crate::{lead_offset_for_kind, NotificationKind, NotificationLeadTimes, ScheduledNotification};

/// Compute notifications scheduled to fire in [from, to).
pub fn compute_due_notifications(
    workspace: &Workspace,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<ScheduledNotification> {
    compute_due_notifications_with_lead_times(workspace, NotificationLeadTimes::default(), from, to)
}

/// Compute notifications scheduled to fire in [from, to) using caller-provided
/// default lead times. Per-occurrence notification offsets still override these
/// defaults.
pub fn compute_due_notifications_with_lead_times(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<ScheduledNotification> {
    compute_due_notifications_with_expander(workspace, lead_times, from, to, &DefaultExpander)
}

pub fn compute_due_notifications_with_expander(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    expander: &dyn OccurrenceExpander,
) -> Vec<ScheduledNotification> {
    let mut out = Vec::new();
    let range = expansion_range(lead_times, from, to);
    for scheme in workspace.iter_schemes() {
        for item in &scheme.items {
            for occurrence in expander.expand(item, range) {
                if let Some(note) =
                    scheduled_notification(scheme.id, item, occurrence, lead_times, from, to, true)
                {
                    out.push(note);
                }
            }
        }
    }
    out.sort_by_key(|n| n.fire_at);
    out
}

/// Compute notification identifiers associated with one item in [from, to).
/// Unlike `compute_due_notifications_with_lead_times`, this includes completed
/// occurrences so callers can clear stale delivered notifications after delete.
pub fn notification_keys_for_item(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    scheme_id: SchemeId,
    item_id: knotq_model::ItemId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<String> {
    notification_keys_for_item_with_expander(
        workspace,
        lead_times,
        scheme_id,
        item_id,
        from,
        to,
        &DefaultExpander,
    )
}

pub fn notification_keys_for_item_with_expander(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    scheme_id: SchemeId,
    item_id: knotq_model::ItemId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    expander: &dyn OccurrenceExpander,
) -> Vec<String> {
    let range = expansion_range(lead_times, from, to);
    let mut out = Vec::new();
    for scheme in workspace
        .iter_schemes()
        .filter(|scheme| scheme.id == scheme_id)
    {
        for item in scheme.items.iter().filter(|item| item.id == item_id) {
            for occurrence in expander.expand(item, range) {
                if let Some(note) =
                    scheduled_notification(scheme.id, item, occurrence, lead_times, from, to, false)
                {
                    out.push(note.key);
                }
            }
        }
    }
    out
}

fn expansion_range(
    lead_times: NotificationLeadTimes,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> DateRange {
    let max_default_lead_secs = [
        lead_times.reminder_offset_secs,
        lead_times.event_offset_secs,
        lead_times.assignment_offset_secs,
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
    .max(0);

    DateRange {
        start: from - Duration::days(370),
        end: to + Duration::seconds(max_default_lead_secs) + Duration::days(2),
    }
}

fn scheduled_notification(
    scheme_id: SchemeId,
    item: &Item,
    occurrence: Occurrence,
    lead_times: NotificationLeadTimes,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    skip_completed: bool,
) -> Option<ScheduledNotification> {
    let kind = NotificationKind::from_item(occurrence.kind)?;
    if skip_completed && occurrence.state.is_done() {
        return None;
    }
    let trigger = trigger_at(kind, &occurrence)?;
    let lead = occurrence
        .state
        .notification_offset_secs
        .map(Duration::seconds)
        .unwrap_or_else(|| lead_offset_for_kind(kind, lead_times));
    let fire_at = trigger - lead;
    if fire_at < from || fire_at >= to {
        return None;
    }
    Some(ScheduledNotification {
        key: ScheduledNotification::make_key(scheme_id, &occurrence.id, kind, fire_at),
        fire_at,
        title: title_for(item),
        body: body_for(kind, occurrence.start, occurrence.end),
        kind,
        trigger_at: trigger,
        scheme_id,
        item_id: item.id,
        occurrence: occurrence.id,
    })
}

fn trigger_at(kind: NotificationKind, occurrence: &Occurrence) -> Option<DateTime<Utc>> {
    match kind {
        NotificationKind::Reminder | NotificationKind::Event => occurrence.start,
        NotificationKind::Assignment => occurrence.end,
    }
}
