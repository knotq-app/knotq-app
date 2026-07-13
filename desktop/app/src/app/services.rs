use std::collections::HashMap;
use std::time::Duration as StdDuration;

use async_channel::{Receiver, Sender};
use knotq_model::{Item, ItemId, OccurrenceId, SchemeId};
use knotq_storage_json::NotificationDefaults;

pub(super) use super::{save_workspace, save_workspace_incremental, workspace_path, KnotQApp};
use crate::app::sync_service::SyncSignal;

mod bus;
mod shutdown;
mod tasks;

pub(crate) use tasks::{spawn_notification_task, spawn_save_task, spawn_timeline_task};

pub(super) const SAVE_DEBOUNCE: StdDuration = StdDuration::from_secs(2);
/// Backoff before re-signalling a save after a failed write (disk full,
/// permission denied, external drive gone, AV lock). Avoids hammering a
/// persistently broken disk while still recovering automatically once it
/// clears, rather than leaving the failed edits stale until the user happens
/// to make another edit.
pub(super) const SAVE_RETRY_BACKOFF: StdDuration = StdDuration::from_secs(10);
pub(super) const NOTIFICATION_DEBOUNCE: StdDuration = StdDuration::from_secs(4);
pub(super) const TIMELINE_POLL_INTERVAL: StdDuration = StdDuration::from_secs(5 * 60);
pub(super) const DEADLINE_LOOKBACK_DAYS: i64 = 7;
pub(super) const DEADLINE_LOOKAHEAD_DAYS: i64 = 370;

#[derive(Clone)]
pub(crate) struct AppServiceBus {
    pub(super) save_tx: Sender<()>,
    pub(super) notification_tx: Sender<NotificationSignal>,
    pub(super) timeline_tx: Sender<()>,
    pub(super) sync_tx: Sender<SyncSignal>,
    pub(super) notification_recompute_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Bumped whenever a change that can affect the notification schedule is
    /// signalled (the same precise condition that triggers an OS-notification
    /// recompute). The sync run reads it to decide whether the schedule it computed
    /// last time is still valid, so a burst of edits that don't touch any dated item
    /// (e.g. typing prose) reuses the cached schedule instead of re-expanding
    /// recurrences and re-hashing every occurrence each run.
    pub(super) notification_schedule_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) struct AppServiceReceivers {
    pub(crate) save_rx: Receiver<()>,
    pub(crate) notification_rx: Receiver<NotificationSignal>,
    pub(crate) timeline_rx: Receiver<()>,
    pub(crate) sync_rx: Receiver<SyncSignal>,
}

#[derive(Clone)]
pub(crate) enum NotificationSignal {
    Recompute,
    RefreshItem(NotificationItemRefresh),
    ClearItem(NotificationItemRefresh),
    ClearOccurrence(NotificationOccurrenceClear),
    Action,
}

#[derive(Clone)]
pub(crate) struct NotificationItemRefresh {
    pub(crate) scheme_id: SchemeId,
    /// Whether `scheme_id` is a daily-queue scheme. Item-level refresh/clear runs
    /// against a synthetic one-scheme workspace, which would otherwise lose the
    /// daily-ness that selects the stable "daily" notification-key fragment.
    pub(crate) scheme_is_daily: bool,
    pub(crate) item: Item,
    pub(crate) defaults: NotificationDefaults,
}

#[derive(Clone)]
pub(crate) struct NotificationOccurrenceClear {
    pub(crate) scheme_id: SchemeId,
    /// See [`NotificationItemRefresh::scheme_is_daily`].
    pub(crate) scheme_is_daily: bool,
    pub(crate) item: Item,
    pub(crate) occurrence: OccurrenceId,
    pub(crate) defaults: NotificationDefaults,
}

#[derive(Default)]
struct NotificationBatch {
    pub(super) needs_recompute: bool,
    pub(super) has_actions: bool,
    pub(super) item_refreshes: HashMap<(SchemeId, ItemId), NotificationItemRefresh>,
    pub(super) item_clears: HashMap<(SchemeId, ItemId), NotificationItemRefresh>,
    pub(super) occurrence_clears:
        HashMap<(SchemeId, ItemId, OccurrenceId), NotificationOccurrenceClear>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use knotq_model::{
        CalendarProvider, ImportedCalendarSource, Item, NodeRef, Scheme, SchemeSource, Workspace,
    };

    use tasks::{next_event_completion_deadline, timeline_wait_until};

    #[test]
    fn timeline_wait_is_capped_for_distant_deadlines() {
        let now = Utc.with_ymd_and_hms(2026, 5, 18, 8, 0, 0).unwrap();
        let deadline = now + Duration::hours(4);

        assert_eq!(timeline_wait_until(deadline, now), TIMELINE_POLL_INTERVAL);
    }

    #[test]
    fn timeline_wait_uses_near_deadline() {
        let now = Utc.with_ymd_and_hms(2026, 5, 18, 8, 0, 0).unwrap();
        let deadline = now + Duration::seconds(15);

        assert_eq!(
            timeline_wait_until(deadline, now),
            StdDuration::from_secs(15)
        );
    }

    #[test]
    fn timeline_wait_is_zero_for_due_deadlines() {
        let now = Utc.with_ymd_and_hms(2026, 5, 18, 8, 0, 0).unwrap();
        let deadline = now - Duration::seconds(1);

        assert_eq!(timeline_wait_until(deadline, now), StdDuration::ZERO);
    }

    #[test]
    fn next_event_completion_deadline_includes_read_only_future_events() {
        let now = Utc.with_ymd_and_hms(2026, 5, 18, 8, 0, 0).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 5, 18, 9, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 5, 18, 10, 0, 0).unwrap();
        let mut workspace = Workspace::new();
        let mut scheme = Scheme::new("Imported", 0);
        scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "acct".into(),
            account_email: None,
            calendar_id: "cal".into(),
            sync_token: None,
            read_only: true,
            last_synced_at: None,
        });
        let scheme_id = scheme.id;
        scheme
            .items
            .push(Item::new("Class").with_start(start).with_end(end));
        workspace.schemes.insert(scheme_id, scheme);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));

        assert_eq!(next_event_completion_deadline(&workspace, now), Some(end));
    }

    #[test]
    fn next_event_completion_deadline_skips_old_events() {
        let now = Utc.with_ymd_and_hms(2026, 5, 18, 8, 0, 0).unwrap();
        let start = now - Duration::days(8);
        let end = start + Duration::hours(1);
        let mut workspace = Workspace::new();
        let mut scheme = Scheme::new("Local", 0);
        let scheme_id = scheme.id;
        scheme
            .items
            .push(Item::new("Class").with_start(start).with_end(end));
        workspace.schemes.insert(scheme_id, scheme);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));

        assert_eq!(next_event_completion_deadline(&workspace, now), None);
    }
}
