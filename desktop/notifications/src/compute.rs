use chrono::{DateTime, Duration, Utc};
use knotq_date_util::DateRange;
use knotq_model::{Item, Occurrence, OccurrenceId, SchemeId, Workspace};
use knotq_rrule::{DefaultExpander, OccurrenceExpander};

use crate::format::{body_for, title_for};
use crate::{lead_offset_for_kind, NotificationKind, NotificationLeadTimes, ScheduledNotification};

const NOTIFICATION_LOOKBACK_DAYS: i64 = 7;

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
        let is_daily = scheme_is_daily(workspace, scheme.id);
        for item in &scheme.items {
            for occurrence in expander.expand(item, range) {
                if let Some(note) = scheduled_notification(
                    scheme.id, is_daily, item, occurrence, lead_times, from, to, true,
                ) {
                    out.push(note);
                }
            }
        }
    }
    out.sort_by(|a, b| a.fire_at.cmp(&b.fire_at).then_with(|| a.key.cmp(&b.key)));
    // Collapse notifications that render as the same banner — same scheme, kind,
    // timing, and text — but originate from distinct items. That is exactly the
    // shape of a duplicated daily row (a carryover that re-ran on view/reload, or
    // the same row rolled forward independently on two devices): two rows with
    // fresh ids would otherwise schedule two identical banners. Keep one
    // deterministic representative (smallest key at a given fire time) so every
    // device drops the same duplicate and cross-device clearing stays consistent.
    let mut seen = std::collections::HashSet::new();
    out.retain(|note| {
        seen.insert((
            note.scheme_id,
            note.kind,
            note.fire_at,
            note.trigger_at,
            note.title.clone(),
            note.body.clone(),
        ))
    });
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
        let is_daily = scheme_is_daily(workspace, scheme.id);
        for item in scheme.items.iter().filter(|item| item.id == item_id) {
            for occurrence in expander.expand(item, range) {
                if let Some(note) = scheduled_notification(
                    scheme.id, is_daily, item, occurrence, lead_times, from, to, false,
                ) {
                    out.push(note.key);
                }
            }
        }
    }
    out
}

/// Compute notification identifiers associated with one concrete occurrence in
/// [from, to). This includes completed occurrences so callers can dismiss stale
/// live notifications for the exact instance that was completed.
pub fn notification_keys_for_occurrence(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    scheme_id: SchemeId,
    item_id: knotq_model::ItemId,
    occurrence_id: &OccurrenceId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<String> {
    notification_keys_for_occurrence_with_expander(
        workspace,
        lead_times,
        scheme_id,
        item_id,
        occurrence_id,
        from,
        to,
        &DefaultExpander,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn notification_keys_for_occurrence_with_expander(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    scheme_id: SchemeId,
    item_id: knotq_model::ItemId,
    occurrence_id: &OccurrenceId,
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
        let is_daily = scheme_is_daily(workspace, scheme.id);
        for item in scheme.items.iter().filter(|item| item.id == item_id) {
            for occurrence in expander
                .expand(item, range)
                .into_iter()
                .filter(|occurrence| &occurrence.id == occurrence_id)
            {
                if let Some(note) = scheduled_notification(
                    scheme.id, is_daily, item, occurrence, lead_times, from, to, false,
                ) {
                    out.push(note.key);
                }
            }
        }
    }
    out
}

/// Compute notification identifiers for completed occurrences in [from, to).
/// This is used during full reconciliation to dismiss delivered notifications
/// that no longer have a desired schedule entry.
pub fn completed_notification_keys(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<String> {
    completed_notification_keys_with_expander(workspace, lead_times, from, to, &DefaultExpander)
}

pub fn completed_notification_keys_with_expander(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    expander: &dyn OccurrenceExpander,
) -> Vec<String> {
    let range = expansion_range(lead_times, from, to);
    let mut out = Vec::new();
    for scheme in workspace.iter_schemes() {
        let is_daily = scheme_is_daily(workspace, scheme.id);
        for item in &scheme.items {
            for occurrence in expander.expand(item, range) {
                if !occurrence.state.is_done() {
                    continue;
                }
                if let Some(note) = scheduled_notification(
                    scheme.id, is_daily, item, occurrence, lead_times, from, to, false,
                ) {
                    out.push(note.key);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Compute delivered event notification identifiers that should no longer be
/// visible because the event occurrence has passed its end time.
pub fn expired_event_notification_keys(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    now: DateTime<Utc>,
) -> Vec<String> {
    expired_event_notification_keys_with_expander(workspace, lead_times, now, &DefaultExpander)
}

pub fn expired_event_notification_keys_with_expander(
    workspace: &Workspace,
    lead_times: NotificationLeadTimes,
    now: DateTime<Utc>,
    expander: &dyn OccurrenceExpander,
) -> Vec<String> {
    let mut out = Vec::new();

    for scheme in workspace.iter_schemes() {
        let is_daily = scheme_is_daily(workspace, scheme.id);
        let range = DateRange {
            start: now - Duration::days(NOTIFICATION_LOOKBACK_DAYS),
            end: now + Duration::seconds(1),
        };
        for item in &scheme.items {
            for occurrence in expander.expand(item, range) {
                if NotificationKind::from_item(occurrence.kind) != Some(NotificationKind::Event) {
                    continue;
                }
                if notification_expires_at(NotificationKind::Event, &occurrence)
                    .is_none_or(|expires_at| expires_at > now)
                {
                    continue;
                }
                let Some(fire_at) =
                    notification_fire_at(NotificationKind::Event, &occurrence, lead_times)
                else {
                    continue;
                };
                if fire_at > now {
                    continue;
                }
                out.push(ScheduledNotification::make_key(
                    scheme.id,
                    is_daily,
                    item.id,
                    &occurrence.id,
                    NotificationKind::Event,
                ));
            }
        }
    }

    out.sort();
    out.dedup();
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
        start: from - Duration::days(NOTIFICATION_LOOKBACK_DAYS),
        end: to + Duration::seconds(max_default_lead_secs) + Duration::days(2),
    }
}

/// Whether `scheme_id` is one of the workspace's per-day daily-queue schemes —
/// these get the stable "daily" key fragment so notification identity survives
/// the rollover hop from one day's scheme to the next.
fn scheme_is_daily(workspace: &Workspace, scheme_id: SchemeId) -> bool {
    workspace.is_daily_queue_scheme(scheme_id)
}

#[allow(clippy::too_many_arguments)]
fn scheduled_notification(
    scheme_id: SchemeId,
    scheme_is_daily: bool,
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
    let fire_at = notification_fire_at(kind, &occurrence, lead_times)?;
    let expires_at = notification_expires_at(kind, &occurrence);
    if expires_at.is_some_and(|expires_at| expires_at <= fire_at) {
        return None;
    }
    if fire_at < from || fire_at >= to {
        return None;
    }
    Some(ScheduledNotification {
        key: ScheduledNotification::make_key(
            scheme_id,
            scheme_is_daily,
            item.id,
            &occurrence.id,
            kind,
        ),
        fire_at,
        expires_at,
        end_at: event_end_at(kind, &occurrence),
        title: title_for(item),
        body: body_for(kind, occurrence.start, occurrence.end),
        kind,
        trigger_at: trigger_at(kind, &occurrence)?,
        scheme_id,
        item_id: item.id,
        occurrence: occurrence.id,
    })
}

fn notification_fire_at(
    kind: NotificationKind,
    occurrence: &Occurrence,
    lead_times: NotificationLeadTimes,
) -> Option<DateTime<Utc>> {
    let trigger = trigger_at(kind, occurrence)?;
    let lead = occurrence
        .state
        .notification_offset_secs
        .map(Duration::seconds)
        .unwrap_or_else(|| lead_offset_for_kind(kind, lead_times));
    Some(trigger - lead)
}

fn notification_expires_at(
    kind: NotificationKind,
    occurrence: &Occurrence,
) -> Option<DateTime<Utc>> {
    match kind {
        NotificationKind::Event => occurrence.end,
        NotificationKind::Reminder | NotificationKind::Assignment => None,
    }
}

fn event_end_at(kind: NotificationKind, occurrence: &Occurrence) -> Option<DateTime<Utc>> {
    match kind {
        NotificationKind::Event => occurrence.end,
        NotificationKind::Reminder | NotificationKind::Assignment => None,
    }
}

fn trigger_at(kind: NotificationKind, occurrence: &Occurrence) -> Option<DateTime<Utc>> {
    match kind {
        NotificationKind::Reminder | NotificationKind::Event => occurrence.start,
        NotificationKind::Assignment => occurrence.end,
    }
}
