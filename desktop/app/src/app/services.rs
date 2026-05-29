use std::collections::HashMap;
use std::time::Duration as StdDuration;

use async_channel::{Receiver, Sender};
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use futures::{pin_mut, select, FutureExt};
use gpui::{Context, Task};
use knotq_model::{Item, ItemId, ItemKind, SchemeId, Workspace};
use knotq_rrule::ItemOccurrenceExt;
use knotq_storage_json::{save_pending_crdt_edits, NotificationDefaults};

use super::{save_workspace, save_workspace_incremental, workspace_path, KnotQApp};

const SAVE_DEBOUNCE: StdDuration = StdDuration::from_secs(2);
const NOTIFICATION_DEBOUNCE: StdDuration = StdDuration::from_secs(4);
const TIMELINE_POLL_INTERVAL: StdDuration = StdDuration::from_secs(5 * 60);
const DEADLINE_LOOKBACK_DAYS: i64 = 7;
const DEADLINE_LOOKAHEAD_DAYS: i64 = 370;

#[derive(Clone)]
pub(crate) struct AppServiceBus {
    save_tx: Sender<()>,
    notification_tx: Sender<NotificationSignal>,
    timeline_tx: Sender<()>,
    sync_tx: Sender<()>,
    notification_recompute_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub(crate) struct AppServiceReceivers {
    pub(crate) save_rx: Receiver<()>,
    pub(crate) notification_rx: Receiver<NotificationSignal>,
    pub(crate) timeline_rx: Receiver<()>,
    pub(crate) sync_rx: Receiver<()>,
}

#[derive(Clone)]
pub(crate) enum NotificationSignal {
    Recompute,
    RefreshItem(NotificationItemRefresh),
    ClearItem(NotificationItemRefresh),
    Action,
}

#[derive(Clone)]
pub(crate) struct NotificationItemRefresh {
    pub(crate) scheme_id: SchemeId,
    pub(crate) item: Item,
    pub(crate) defaults: NotificationDefaults,
}

impl AppServiceBus {
    pub(crate) fn new() -> (Self, AppServiceReceivers) {
        let (save_tx, save_rx) = async_channel::bounded(1);
        let (notification_tx, notification_rx) = async_channel::unbounded();
        let (timeline_tx, timeline_rx) = async_channel::bounded(1);
        let (sync_tx, sync_rx) = async_channel::bounded(1);
        (
            Self {
                save_tx,
                notification_tx,
                timeline_tx,
                sync_tx,
                notification_recompute_pending: std::sync::Arc::new(
                    std::sync::atomic::AtomicBool::new(false),
                ),
            },
            AppServiceReceivers {
                save_rx,
                notification_rx,
                timeline_rx,
                sync_rx,
            },
        )
    }

    pub(crate) fn workspace_changed(&self) {
        self.signal_save();
        self.signal_notifications();
        self.signal_timeline();
        self.signal_sync();
    }

    pub(crate) fn signal_save(&self) {
        let _ = self.save_tx.try_send(());
    }

    pub(crate) fn signal_notifications(&self) {
        if !self
            .notification_recompute_pending
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            let _ = self.notification_tx.try_send(NotificationSignal::Recompute);
        }
    }

    pub(crate) fn signal_item_notifications(
        &self,
        scheme_id: SchemeId,
        item: Item,
        defaults: NotificationDefaults,
    ) {
        // Skip item-level refresh if a full recompute is already pending.
        if self
            .notification_recompute_pending
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let _ = self
            .notification_tx
            .try_send(NotificationSignal::RefreshItem(NotificationItemRefresh {
                scheme_id,
                item,
                defaults,
            }));
    }

    pub(crate) fn signal_clear_item_notifications(
        &self,
        scheme_id: SchemeId,
        item: Item,
        defaults: NotificationDefaults,
    ) {
        let _ =
            self.notification_tx
                .try_send(NotificationSignal::ClearItem(NotificationItemRefresh {
                    scheme_id,
                    item,
                    defaults,
                }));
    }

    pub(crate) fn signal_timeline(&self) {
        let _ = self.timeline_tx.try_send(());
    }

    pub(crate) fn signal_sync(&self) {
        let _ = self.sync_tx.try_send(());
    }

    fn signal_notification_action(&self) -> bool {
        self.notification_tx
            .try_send(NotificationSignal::Action)
            .is_ok()
    }

    fn clear_notification_recompute_pending(&self) {
        self.notification_recompute_pending
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

pub(crate) fn spawn_save_task(save_rx: Receiver<()>, cx: &mut Context<KnotQApp>) -> Task<()> {
    cx.spawn(
        async move |weak: gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp| {
            while save_rx.recv().await.is_ok() {
                cx.background_executor().timer(SAVE_DEBOUNCE).await;
                drain_unit_signals(&save_rx);

                let snapshot = weak
                    .update(cx, |app, _cx| {
                        if app.workspace_save_blocked_reason.is_some() {
                            return None;
                        }
                        if !app.state.is_dirty() {
                            return None;
                        }
                        let pending_crdt_edits = app.state.pending_crdt_edits();
                        let dirty_ids = std::mem::take(&mut app.state.dirty_schemes);
                        app.state.index_dirty = false;
                        Some((app.workspace.clone(), dirty_ids, pending_crdt_edits))
                    })
                    .ok()
                    .flatten();

                if let Some((ws, dirty_ids, pending_crdt_edits)) = snapshot {
                    let path = workspace_path();
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            let result = if dirty_ids.is_empty() {
                                save_workspace(&path, &ws)
                            } else {
                                save_workspace_incremental(&path, &ws, &dirty_ids)
                            };
                            result.and_then(|_| save_pending_crdt_edits(&path, &pending_crdt_edits))
                        })
                        .await;
                    if let Err(err) = result {
                        eprintln!("save failed: {err:#}");
                    }
                }
            }
        },
    )
}

pub(crate) fn spawn_notification_task(
    bus: AppServiceBus,
    notification_rx: Receiver<NotificationSignal>,
    cx: &mut Context<KnotQApp>,
) -> Task<()> {
    knotq_notifications::add_notification_response_listener({
        let bus = bus.clone();
        move || bus.signal_notification_action()
    });

    cx.spawn(
        async move |weak: gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp| {
            crate::notifications::notif_log("notification service started");

            handle_notification_actions(&weak, cx);
            cx.background_executor()
                .timer(StdDuration::from_secs(3))
                .await;
            handle_notification_actions(&weak, cx);
            update_notification_error(&weak, cx, None);
            refresh_os_notifications(&weak, cx).await;

            // Drain any signals that queued during the startup refresh to
            // avoid a redundant second reconciliation.
            bus.clear_notification_recompute_pending();
            while notification_rx.try_recv().is_ok() {}

            while let Ok(signal) = notification_rx.recv().await {
                let mut batch = NotificationBatch::default();
                batch.push(signal);
                if batch.needs_recompute
                    || !batch.item_refreshes.is_empty()
                    || !batch.item_clears.is_empty()
                {
                    cx.background_executor().timer(NOTIFICATION_DEBOUNCE).await;
                }
                while let Ok(signal) = notification_rx.try_recv() {
                    batch.push(signal);
                }
                if batch.needs_recompute {
                    bus.clear_notification_recompute_pending();
                }

                if batch.has_actions {
                    handle_notification_actions(&weak, cx);
                }
                if !batch.item_clears.is_empty() {
                    clear_item_os_notifications(batch.item_clears, cx).await;
                }
                if batch.needs_recompute {
                    refresh_os_notifications(&weak, cx).await;
                } else if !batch.item_refreshes.is_empty() {
                    refresh_item_os_notifications(batch.item_refreshes, &weak, cx).await;
                }
            }
        },
    )
}

pub(crate) fn spawn_timeline_task(
    timeline_rx: Receiver<()>,
    cx: &mut Context<KnotQApp>,
) -> Task<()> {
    cx.spawn(
        async move |weak: gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp| loop {
            sync_daily_queue_day_boundary_if_needed(&weak, cx);
            let deadline = compute_next_timeline_deadline(&weak, cx).await;
            let mut should_run_due_jobs = false;

            match deadline {
                Some(deadline) if deadline > Utc::now() => {
                    let wait = timeline_wait_until(deadline, Utc::now());
                    let timer = cx.background_executor().timer(wait).fuse();
                    let signal = timeline_rx.recv().fuse();
                    pin_mut!(timer, signal);
                    select! {
                        _ = timer => {
                            should_run_due_jobs = deadline <= Utc::now();
                        }
                        result = signal => {
                            if result.is_err() {
                                break;
                            }
                            should_run_due_jobs = true;
                        }
                    }
                }
                Some(_) => {
                    should_run_due_jobs = true;
                }
                None => {
                    let timer = cx
                        .background_executor()
                        .timer(TIMELINE_POLL_INTERVAL)
                        .fuse();
                    let signal = timeline_rx.recv().fuse();
                    pin_mut!(timer, signal);
                    select! {
                        _ = timer => {}
                        result = signal => {
                            if result.is_err() {
                                break;
                            }
                            should_run_due_jobs = true;
                        }
                    }
                }
            }
            should_run_due_jobs |= drain_unit_signals(&timeline_rx);

            if should_run_due_jobs {
                run_due_timeline_jobs(&weak, cx).await;
            }
        },
    )
}

fn timeline_wait_until(deadline: DateTime<Utc>, now: DateTime<Utc>) -> StdDuration {
    deadline
        .signed_duration_since(now)
        .to_std()
        .unwrap_or(StdDuration::ZERO)
        .min(TIMELINE_POLL_INTERVAL)
}

pub(crate) fn next_event_completion_deadline(
    workspace: &Workspace,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let scan_start = now - Duration::days(DEADLINE_LOOKBACK_DAYS);
    let scan_end = now + Duration::days(DEADLINE_LOOKAHEAD_DAYS);
    let mut next = None;

    for scheme in workspace.iter_schemes() {
        for item in &scheme.items {
            if item.kind() != ItemKind::Event {
                continue;
            }
            for occurrence in item.occurrences(scan_start, scan_end) {
                if occurrence.kind != ItemKind::Event || occurrence.state.is_done() {
                    continue;
                }
                let Some(end) = occurrence.end else {
                    continue;
                };
                if end <= now {
                    return Some(now);
                }
                next = Some(next.map_or(end, |current: DateTime<Utc>| current.min(end)));
            }
        }
    }

    next
}

pub(crate) fn next_daily_queue_deadline() -> Option<DateTime<Utc>> {
    let next_day = Local::now().date_naive().succ_opt()?;
    let local_midnight = next_day.and_hms_opt(0, 0, 0)?;
    Local
        .from_local_datetime(&local_midnight)
        .earliest()
        .map(|dt| dt.with_timezone(&Utc))
}

async fn refresh_os_notifications(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    let snapshot = weak
        .update(cx, |app, _cx| {
            Some((app.workspace.clone(), app.notification_defaults))
        })
        .ok()
        .flatten();
    let Some((workspace, defaults)) = snapshot else {
        return;
    };

    let schedule_error = cx
        .background_executor()
        .spawn(async move {
            let update = crate::notifications::recompute_pending(&workspace, defaults);
            let schedule_error = crate::notifications::schedule_os_notifications(&update.requests);
            let cleanup_error = crate::notifications::clear_expired_event_notifications(
                &workspace,
                defaults,
                Utc::now(),
            );
            schedule_error.or(cleanup_error)
        })
        .await;
    let _ = weak.update(cx, |app, cx| {
        app.notification_error =
            schedule_error.or_else(crate::notifications::notification_availability_error);
        cx.notify();
    });
}

async fn refresh_item_os_notifications(
    item_refreshes: HashMap<(SchemeId, ItemId), NotificationItemRefresh>,
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
) {
    let first_error = cx
        .background_executor()
        .spawn(async move {
            let mut first_error = None;
            for refresh in item_refreshes.into_values() {
                let err = crate::notifications::refresh_item_os_notifications(
                    refresh.scheme_id,
                    refresh.item,
                    refresh.defaults,
                );
                if first_error.is_none() {
                    first_error = err;
                }
            }
            first_error
        })
        .await;

    if first_error.is_some() {
        update_notification_error(weak, cx, first_error);
    }
}

async fn clear_item_os_notifications(
    item_clears: HashMap<(SchemeId, ItemId), NotificationItemRefresh>,
    cx: &mut gpui::AsyncApp,
) {
    cx.background_executor()
        .spawn(async move {
            for clear in item_clears.into_values() {
                crate::notifications::clear_item_notifications_for_item(
                    clear.scheme_id,
                    clear.item,
                    clear.defaults,
                );
            }
        })
        .await;
}

async fn compute_next_timeline_deadline(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
) -> Option<DateTime<Utc>> {
    let now = Utc::now();
    let workspace = weak.update(cx, |app, _cx| app.workspace.clone()).ok()?;
    let event_deadline = cx
        .background_executor()
        .spawn(async move { next_event_completion_deadline(&workspace, now) })
        .await;

    [event_deadline, next_daily_queue_deadline()]
        .into_iter()
        .flatten()
        .min()
}

fn sync_daily_queue_day_boundary_if_needed(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
) {
    let today = Local::now().date_naive();
    let _ = weak.update(cx, |app, cx| {
        app.sync_daily_queue_day_boundary_to(today, cx);
    });
}

async fn run_due_timeline_jobs(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    let now = Utc::now();
    let workspace = weak.update(cx, |app, _cx| app.workspace.clone()).ok();
    let completion_keys = match workspace {
        Some(workspace) => {
            cx.background_executor()
                .spawn(async move { knotq_state::past_event_completion_keys(&workspace, now) })
                .await
        }
        None => Vec::new(),
    };

    let _ = weak.update(cx, |app, cx| {
        app.complete_past_event_occurrences(&completion_keys, now, cx);
        app.sync_daily_queue_day_boundary(cx);
    });
}

fn handle_notification_actions(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    let targets = crate::notifications::drain_notification_action_targets();
    if targets.is_empty() {
        return;
    }
    let _ = weak.update(cx, |app, cx| {
        app.handle_notification_action_targets(targets, cx);
    });
}

fn update_notification_error(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    error: Option<String>,
) {
    let error = error.or_else(crate::notifications::notification_availability_error);
    let _ = weak.update(cx, |app, cx| {
        app.notification_error = error;
        cx.notify();
    });
}

fn drain_unit_signals(rx: &Receiver<()>) -> bool {
    let mut drained = false;
    while rx.try_recv().is_ok() {
        drained = true;
    }
    drained
}

#[derive(Default)]
struct NotificationBatch {
    needs_recompute: bool,
    has_actions: bool,
    item_refreshes: HashMap<(SchemeId, ItemId), NotificationItemRefresh>,
    item_clears: HashMap<(SchemeId, ItemId), NotificationItemRefresh>,
}

impl NotificationBatch {
    fn push(&mut self, signal: NotificationSignal) {
        match signal {
            NotificationSignal::Recompute => self.needs_recompute = true,
            NotificationSignal::RefreshItem(refresh) => {
                self.item_refreshes
                    .insert((refresh.scheme_id, refresh.item.id), refresh);
            }
            NotificationSignal::ClearItem(clear) => {
                self.item_clears
                    .insert((clear.scheme_id, clear.item.id), clear);
            }
            NotificationSignal::Action => self.has_actions = true,
        }
    }
}

impl KnotQApp {
    pub(crate) fn flush_for_shutdown(&mut self, reason: &str) {
        crate::notifications::notif_log(&format!("shutdown flush started: {reason}"));

        let completed = knotq_state::mark_past_events_done(&mut self.workspace, Utc::now());
        if completed > 0 {
            let all_ids: Vec<_> = self.workspace.schemes.keys().copied().collect();
            for id in all_ids {
                self.dirty_schemes.insert(id);
            }
            self.index_dirty = true;
            self.state.mark_compat_workspace_dirty();
            crate::notifications::notif_log(&format!(
                "shutdown marked {completed} elapsed event occurrence(s) complete"
            ));
        }

        self.save_app_settings();

        if let Some(reason) = &self.workspace_save_blocked_reason {
            crate::notifications::notif_log(&format!(
                "shutdown workspace flush skipped because workspace load failed: {reason}"
            ));
            eprintln!("shutdown workspace flush skipped because workspace load failed: {reason}");
        } else {
            match save_workspace(&workspace_path(), &self.workspace) {
                Ok(()) => {
                    self.dirty_schemes.clear();
                    self.index_dirty = false;
                    crate::notifications::notif_log("shutdown workspace flush completed");
                }
                Err(err) => {
                    crate::notifications::notif_log(&format!(
                        "shutdown workspace flush failed: {err:#}"
                    ));
                    eprintln!("shutdown workspace flush failed: {err:#}");
                }
            }
        }

        let update =
            crate::notifications::recompute_pending(&self.workspace, self.notification_defaults);
        let schedule_error =
            crate::notifications::schedule_os_notifications_for_shutdown(&update.requests);
        let cleanup_error = crate::notifications::clear_expired_event_notifications(
            &self.workspace,
            self.notification_defaults,
            Utc::now(),
        );
        if let Some(err) = schedule_error.or(cleanup_error) {
            crate::notifications::notif_log(&format!(
                "shutdown OS notification schedule flush failed: {err}"
            ));
            eprintln!("shutdown OS notification schedule flush failed: {err}");
            self.notification_error = Some(err);
        } else {
            self.notification_error = crate::notifications::notification_availability_error();
            crate::notifications::notif_log("shutdown OS notification schedule flush completed");
        }

        crate::notifications::notif_log("shutdown flush finished");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::{
        CalendarProvider, ImportedCalendarSource, Item, NodeRef, Scheme, SchemeSource,
    };

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
