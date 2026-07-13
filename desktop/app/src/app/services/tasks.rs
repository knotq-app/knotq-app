use std::collections::HashMap;
use std::time::Duration as StdDuration;

use async_channel::Receiver;
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use futures::{pin_mut, select, FutureExt};
use gpui::{Context, Task};
use knotq_model::{ItemId, ItemKind, OccurrenceId, SchemeId, Workspace};
use knotq_rrule::ItemOccurrenceExt;
use knotq_storage_json::{save_crdt_state, save_pending_crdt_edits};

use super::{
    save_workspace, save_workspace_incremental, workspace_path, AppServiceBus, KnotQApp,
    NotificationBatch, NotificationItemRefresh, NotificationOccurrenceClear, NotificationSignal,
    DEADLINE_LOOKAHEAD_DAYS, DEADLINE_LOOKBACK_DAYS, NOTIFICATION_DEBOUNCE, SAVE_DEBOUNCE,
    SAVE_RETRY_BACKOFF, TIMELINE_POLL_INTERVAL,
};

pub(crate) fn spawn_save_task(
    bus: AppServiceBus,
    save_rx: Receiver<()>,
    cx: &mut Context<KnotQApp>,
) -> Task<()> {
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
                        let crdt_states = app.state.crdt_document_states();
                        let dirty_ids = std::mem::take(&mut app.state.dirty_schemes);
                        app.state.index_dirty = false;
                        Some((
                            app.workspace.clone(),
                            dirty_ids,
                            pending_crdt_edits,
                            crdt_states,
                        ))
                    })
                    .ok()
                    .flatten();

                if let Some((ws, dirty_ids, pending_crdt_edits, crdt_states)) = snapshot {
                    let path = workspace_path();
                    let retry_ids = dirty_ids.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            let result = if dirty_ids.is_empty() {
                                save_workspace(&path, &ws)
                            } else {
                                save_workspace_incremental(&path, &ws, &dirty_ids)
                            };
                            // Persist the CRDT documents' state in lockstep with the
                            // workspace so a restart restores them consistently (and
                            // with their stable identity) rather than rebuilding.
                            result
                                .and_then(|_| save_pending_crdt_edits(&path, &pending_crdt_edits))
                                .and_then(|_| save_crdt_state(&path, &crdt_states))
                        })
                        .await;
                    if let Err(err) = result {
                        eprintln!("save failed: {err:#}");
                        let message = format!("{err:#}");
                        let _ = weak.update(cx, |app, cx| {
                            // Re-mark the schemes this attempt dropped as dirty
                            // (unioned with anything a concurrent edit already
                            // re-added) so the retry below picks them back up
                            // instead of leaving them stale on disk until the
                            // user happens to edit them again.
                            app.state.dirty_schemes.extend(retry_ids);
                            app.state.index_dirty = true;
                            app.workspace_save_error = Some(message);
                            cx.notify();
                        });
                        // Don't hammer a persistently broken disk (full,
                        // permission denied, external drive gone, AV lock); back
                        // off, then self-signal so the loop retries without
                        // requiring another edit to wake it.
                        cx.background_executor().timer(SAVE_RETRY_BACKOFF).await;
                        bus.signal_save();
                    } else {
                        let _ = weak.update(cx, |app, cx| {
                            if app.workspace_save_error.is_some() {
                                app.workspace_save_error = None;
                                cx.notify();
                            }
                        });
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
                    || !batch.occurrence_clears.is_empty()
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
                if !batch.occurrence_clears.is_empty() {
                    clear_occurrence_os_notifications(batch.occurrence_clears, cx).await;
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

pub(super) fn timeline_wait_until(deadline: DateTime<Utc>, now: DateTime<Utc>) -> StdDuration {
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
            let completed_cleanup_error = crate::notifications::clear_completed_notifications(
                &workspace,
                defaults,
                Utc::now(),
            );
            let cleanup_error = crate::notifications::clear_expired_event_notifications(
                &workspace,
                defaults,
                Utc::now(),
            );
            schedule_error.or(completed_cleanup_error).or(cleanup_error)
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
                    refresh.scheme_is_daily,
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
                    clear.scheme_is_daily,
                    clear.item,
                    clear.defaults,
                );
            }
        })
        .await;
}

async fn clear_occurrence_os_notifications(
    occurrence_clears: HashMap<(SchemeId, ItemId, OccurrenceId), NotificationOccurrenceClear>,
    cx: &mut gpui::AsyncApp,
) {
    cx.background_executor()
        .spawn(async move {
            for clear in occurrence_clears.into_values() {
                crate::notifications::clear_occurrence_notifications_for_item(
                    clear.scheme_id,
                    clear.scheme_is_daily,
                    clear.item,
                    clear.occurrence,
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
    let (workspace, retained_deadline) = weak
        .update(cx, |app, _cx| {
            (app.workspace.clone(), app.retained_completed().next_expiry())
        })
        .ok()?;
    let event_deadline = cx
        .background_executor()
        .spawn(async move { next_event_completion_deadline(&workspace, now) })
        .await;

    [event_deadline, next_daily_queue_deadline(), retained_deadline]
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
        // Completed-overdue rows held on the upcoming panel age out after their
        // TTL; the purge re-renders so the row actually leaves the sidebar.
        if app.retained_completed_mut().purge_expired(now) > 0 {
            cx.notify();
        }
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
