use std::time::Duration as StdDuration;

use async_channel::Receiver;
use chrono::Utc;
use futures::{pin_mut, select, FutureExt};
use gpui::{Context, Task};
use knotq_model::SyncAccountStatus;
use knotq_storage_json::{load_local_sync_state, workspace_path};

use super::snapshot::sync_snapshot;
use super::{
    SyncNetworkUnreachable, SyncSignal, SyncSnapshot, SyncUnauthorized, ACCESS_REFRESH_SKEW_SECS,
    SYNC_DEBOUNCE, SYNC_DEBOUNCE_WS, SYNC_LOCAL_CHANGE_DEBOUNCE, SYNC_LOCAL_CHANGE_DEBOUNCE_WS,
    SYNC_PENDING_RETRY, SYNC_POLL_BACKGROUND, SYNC_POLL_FOREGROUND, SYNC_POLL_OFFLINE,
    SYNC_POLL_WS_CONNECTED, SYNC_POLL_WS_IDLE_RECHECK,
};
use crate::app::sync_auth::{refresh_sync_backend, RefreshError};
use crate::app::{KnotQApp, SyncAuthStatus, SyncRunStatus, View};

pub(crate) fn spawn_sync_task(
    sync_rx: Receiver<SyncSignal>,
    cx: &mut Context<KnotQApp>,
) -> Task<()> {
    cx.spawn(
        async move |weak: gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp| {
            // Run once at startup (immediate, no debounce).
            record_poll_at(&weak, cx);
            run_sync_once(&weak, cx).await;
            loop {
                // Choose the timer interval based on current app state.
                let interval = poll_interval(&weak, cx);
                let timer = cx.background_executor().timer(interval).fuse();
                let signal = sync_rx.recv().fuse();
                pin_mut!(timer, signal);
                let mut received_signal: Option<SyncSignal> = None;
                select! {
                    _ = timer => {}
                    result = signal => {
                        match result {
                            Ok(sig) => received_signal = Some(sig),
                            Err(_) => break,
                        }
                    }
                }

                if let Some(first_signal) = received_signal {
                    // Debounce: while the WebSocket is connected these collapse to
                    // ~instant windows (300 ms / 150 ms) since a push has no
                    // per-edit connection cost; on the HTTP fallback they stay long
                    // (30 s / 2 s) so an offline device doesn't reconnect per edit.
                    // During the wait keep listening — an Immediate mid-wait shortens
                    // the deadline to now+immediate (only if sooner), further
                    // LocalChanges do not extend the deadline.
                    let ws_connected = ws_sync_connected(&weak, cx);
                    let immediate_debounce = if ws_connected {
                        SYNC_DEBOUNCE_WS
                    } else {
                        SYNC_DEBOUNCE
                    };
                    let debounce = match first_signal {
                        SyncSignal::Immediate => immediate_debounce,
                        SyncSignal::LocalChange => {
                            if ws_connected {
                                SYNC_LOCAL_CHANGE_DEBOUNCE_WS
                            } else {
                                SYNC_LOCAL_CHANGE_DEBOUNCE
                            }
                        }
                    };
                    let mut debounce_end = std::time::Instant::now() + debounce;
                    loop {
                        let remaining = debounce_end
                            .checked_duration_since(std::time::Instant::now())
                            .unwrap_or(StdDuration::ZERO);
                        if remaining.is_zero() {
                            break;
                        }
                        let timer = cx.background_executor().timer(remaining).fuse();
                        let signal = sync_rx.recv().fuse();
                        pin_mut!(timer, signal);
                        select! {
                            _ = timer => break,
                            result = signal => {
                                match result {
                                    Ok(SyncSignal::Immediate) => {
                                        // Shorten the deadline to the immediate window
                                        // from now, but only if that is sooner than
                                        // the current end.
                                        let shortened =
                                            std::time::Instant::now() + immediate_debounce;
                                        if shortened < debounce_end {
                                            debounce_end = shortened;
                                        }
                                    }
                                    Ok(SyncSignal::LocalChange) => {
                                        // Additional local changes don't extend the wait.
                                    }
                                    Err(_) => return,
                                }
                            }
                        }
                    }
                    // Drain any remaining queued signals.
                    while sync_rx.try_recv().is_ok() {}
                } else if foreground_ws_idle(&weak, cx) {
                    // Bare timer wake in foreground pure-WS mode: this is only a
                    // connectivity re-check, NOT a poll. Skip the network sync and
                    // loop to re-arm the timer (so a dropped socket still resumes
                    // polling within ~one re-check interval). A real signal — a local
                    // edit or a `changed`/on-connect nudge — takes the branch above.
                    continue;
                }

                record_poll_at(&weak, cx);
                run_sync_once(&weak, cx).await;
            }
        },
    )
}

/// Compute the poll-timer interval from the current app state.
///
/// This is a pure function of (has_pending, offline, server_rejecting,
/// window_active) exposed as a separate free function so it can be unit-tested.
pub(crate) fn sync_poll_interval(
    has_pending: bool,
    offline: bool,
    server_rejecting: bool,
    window_active: bool,
) -> StdDuration {
    if has_pending && !server_rejecting {
        // Pending edits exist and the server hasn't rejected them (i.e. either
        // we're offline or haven't tried yet): retry aggressively.
        SYNC_PENDING_RETRY
    } else if offline {
        SYNC_POLL_OFFLINE
    } else if window_active {
        SYNC_POLL_FOREGROUND
    } else {
        SYNC_POLL_BACKGROUND
    }
}

fn poll_interval(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) -> StdDuration {
    weak.update(cx, |app, _cx| {
        let has_pending = app.state.has_pending_crdt_edits() || app.sync_pending_hint > 0;
        let offline = app.sync_offline;
        let server_rejecting = app.sync_server_rejecting;
        let window_active = app.window_is_active;
        // When the socket is live and we're caught up, lean entirely on the
        // server's `changed` nudges + the on-(re)connect catch-up — no network
        // polling. In the foreground the timer is just a short LOCAL re-check tick
        // (the loop skips the actual sync via `foreground_ws_idle`), so a dropped
        // socket is noticed within ~60 s and polling resumes. Backgrounded, keep a
        // slow real heartbeat in case the OS throttles the socket while hidden.
        let ws_connected = app
            .ws_sync
            .as_ref()
            .is_some_and(|client| client.is_connected());
        if ws_connected && !has_pending && !server_rejecting {
            return if window_active {
                SYNC_POLL_WS_IDLE_RECHECK
            } else {
                SYNC_POLL_WS_CONNECTED
            };
        }
        sync_poll_interval(has_pending, offline, server_rejecting, window_active)
    })
    .unwrap_or(SYNC_POLL_FOREGROUND)
}

/// Whether we are in foreground pure-WS mode: window active, socket live, and
/// caught up. In this state the periodic timer must NOT trigger a network sync —
/// real-time convergence is entirely socket-driven (`changed` nudges + the
/// on-(re)connect catch-up), so a bare timer wake is only a connectivity re-check.
fn foreground_ws_idle(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) -> bool {
    weak.update(cx, |app, _cx| {
        let has_pending = app.state.has_pending_crdt_edits() || app.sync_pending_hint > 0;
        let ws_connected = app
            .ws_sync
            .as_ref()
            .is_some_and(|client| client.is_connected());
        app.window_is_active && ws_connected && !has_pending && !app.sync_server_rejecting
    })
    .unwrap_or(false)
}

/// Whether the live WebSocket sync client is currently connected. Used to pick the
/// short ("instant") debounce windows over the long HTTP-fallback ones.
fn ws_sync_connected(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) -> bool {
    weak.update(cx, |app, _cx| {
        app.ws_sync
            .as_ref()
            .is_some_and(|client| client.is_connected())
    })
    .unwrap_or(false)
}

fn record_poll_at(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    let now = Utc::now();
    let _ = weak.update(cx, |app, _cx| {
        app.last_sync_poll_at = Some(now);
    });
}

async fn run_sync_once(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    // Refresh the (short-lived) access token before syncing if it's near expiry,
    // persisting the rotated credentials immediately so a rotated refresh token is
    // never lost to a later sync failure. Aborts this tick if the session is gone.
    if ensure_fresh_token(weak, cx, false).await.is_err() {
        return;
    }

    // At most one reactive force-refresh per run: the proactive expiry check above
    // can't catch a token the server rejects early (revocation, key rotation, clock
    // skew), so an `unauthorized` rejection triggers a forced refresh + one retry
    // instead of surfacing "sync failed: unauthorized" to a signed-in user. Mirrors
    // the mobile shells' bounded refresh-retry.
    let mut tried_auth_refresh = false;
    loop {
        // With the retry spent, an unauthorized failure surfaces as a normal sync
        // error inside the attempt, which then reports `Done`.
        if run_sync_attempt(weak, cx, !tried_auth_refresh).await == AttemptOutcome::Done {
            return;
        }
        tried_auth_refresh = true;
        eprintln!("sync rejected as unauthorized; force-refreshing token and retrying");
        if ensure_fresh_token(weak, cx, true).await.is_err() {
            return;
        }
        // Tear down the WebSocket client: its session was authenticated with the
        // rejected token. The retry's `ensure_ws_sync` rebuilds it immediately, so
        // the new handshake presents the fresh token instead of waiting out the
        // supervisor's backoff on the dead credential (which would leave the app
        // parked on HTTP polling).
        let _ = weak.update(cx, |app, _cx| app.teardown_ws_sync());
    }
}

#[derive(Eq, PartialEq)]
enum AttemptOutcome {
    /// The attempt ran to a final state (success, skip, or a surfaced error).
    Done,
    /// The backend rejected the token and `allow_auth_retry` was set: no error
    /// state was written (the run status stays `Running`) so the caller's forced
    /// refresh + retry can resolve it invisibly.
    Unauthorized,
}

async fn run_sync_attempt(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    allow_auth_retry: bool,
) -> AttemptOutcome {
    let snapshot = weak
        .update(cx, |app, _cx| {
            if app.workspace_save_blocked_reason.is_some() {
                return None;
            }
            // Create/refresh (or tear down) the WebSocket sync client to match the
            // current account before snapshotting, so the run can prefer it.
            app.ensure_ws_sync();
            let account = app.settings.sync_account.clone()?;
            if !account.supports_sync {
                app.sync_run_status = SyncRunStatus::Idle;
                _cx.notify();
                return None;
            }
            app.state.sync_store_from_workspace();
            let pending = app.state.pending_crdt_edits();
            let crdt_states = app.state.crdt_document_states();
            app.sync_run_status = SyncRunStatus::Running {
                pending: pending.len(),
            };
            _cx.notify();
            // Computing the notification schedule (recurrence expansion +
            // per-occurrence hashing) is the heaviest step of building this snapshot,
            // so it runs on the background sync thread, never on main. Beyond that,
            // reuse the previous run's schedule outright when nothing that could
            // change it has happened since: the generation counter only advances on a
            // schedule-relevant signal, so a burst of edits that touch no dated item
            // (typing prose) skips the recompute entirely. The reuse decision here is
            // O(1); `None` makes the background thread recompute.
            let notification_defaults = app.settings.notification_defaults;
            let schedule_gen = app.service_bus.notification_schedule_generation();
            let today = Utc::now().date_naive();
            let reuse_schedule = app
                .cached_notification_schedule
                .as_ref()
                .filter(|cache| {
                    cache.generation == schedule_gen
                        && cache.defaults == notification_defaults
                        && cache.snapshot.window_start.date_naive() == today
                })
                .map(|cache| cache.snapshot.clone());
            let snapshot = SyncSnapshot {
                workspace: app.workspace.clone(),
                account,
                replica_id: app.settings.replica_id,
                pending,
                crdt_states,
                notification_defaults,
                reuse_schedule,
                ws_sync: app.ws_sync.clone(),
            };
            // Captured after sync_store_from_workspace above, so it only moves
            // again if the user edits while the run is in flight.
            Some((
                snapshot,
                app.state.local_edit_watermark(),
                schedule_gen,
                notification_defaults,
            ))
        })
        .ok()
        .flatten();

    let Some((snapshot, local_edit_watermark, schedule_gen, notification_defaults)) = snapshot
    else {
        return AttemptOutcome::Done;
    };

    let result = cx
        .background_executor()
        .spawn(async move { sync_snapshot(snapshot) })
        .await;

    match result {
        Ok(result) => {
            let remote_updates_applied = result.remote_updates_applied;
            let pushed = result.pushed.clone();
            let workspace = result.workspace.clone();
            let crdt_states = result.crdt_states.clone();
            let remaining_pending = result.remaining_pending;
            let local_workspace_changed = result.local_workspace_changed;
            let media_downloaded = result.media_downloaded;
            let notification_schedule = result.notification_schedule.clone();
            let _ = weak.update(cx, |app, cx| {
                for pushed in pushed {
                    app.state
                        .clear_pushed_crdt_edits(pushed.document, pushed.through_local_sequence);
                }
                // Cache the schedule this run used against the generation it was
                // computed at, so the next run can skip recomputing it when nothing
                // schedule-relevant has changed since. If an edit bumped the
                // generation while this run was in flight, the stored generation no
                // longer matches and the next run recomputes — no stale schedule.
                app.cached_notification_schedule =
                    Some(crate::app::sync_service::CachedNotificationSchedule {
                        generation: schedule_gen,
                        defaults: notification_defaults,
                        snapshot: notification_schedule,
                    });
                app.sync_run_status = SyncRunStatus::Synced {
                    pending: remaining_pending,
                };
                app.sync_pending_hint = remaining_pending;
                app.sync_offline = false;
                app.sync_server_rejecting = false;
                app.last_synced_at = Some(Utc::now());
                if remote_updates_applied > 0 || local_workspace_changed {
                    let scheme_scroll_restore = if app.selection.view == View::Scheme {
                        app.selection
                            .scheme_id
                            .map(|scheme_id| (scheme_id, app.scheme_scroll_handle.offset()))
                    } else {
                        None
                    };
                    let daily_queue_scroll_restore = (app.selection.view == View::DailyQueue)
                        .then(|| app.daily_queue_scroll_handle.offset());
                    // Edits applied while the run was in flight are not in its
                    // result; merge the result into the live documents so they
                    // survive (e.g. an event being drafted on the calendar)
                    // instead of being rolled back until the next round trip.
                    // With no in-flight edits the replace is equivalent and
                    // adopts the run's canonical merged state wholesale.
                    let merged = app.state.has_local_edits_since(local_edit_watermark)
                        && app
                            .state
                            .merge_workspace_from_sync(&workspace, &crdt_states);
                    if !merged {
                        app.state
                            .replace_workspace_from_sync(workspace, crdt_states);
                    }
                    app.scheme_scroll_restore_after_sync = scheme_scroll_restore;
                    app.daily_queue_scroll_restore_after_sync = daily_queue_scroll_restore;
                    app.service_bus.signal_save();
                    app.service_bus.signal_notifications();
                    app.service_bus.signal_timeline();
                }
                if media_downloaded {
                    // Drop cached load failures so freshly downloaded assets are
                    // re-read on the repaint below instead of staying blank.
                    if let Some((_, editor)) = app.scheme_editor.clone() {
                        editor.update(cx, |editor, cx| {
                            editor.forget_missing_images();
                            cx.notify();
                        });
                    }
                    for editor in app
                        .daily_queue_editors
                        .values()
                        .cloned()
                        .collect::<Vec<_>>()
                    {
                        editor.update(cx, |editor, cx| {
                            editor.forget_missing_images();
                            cx.notify();
                        });
                    }
                    app.service_bus.signal_timeline();
                }
                cx.notify();
            });
            AttemptOutcome::Done
        }
        Err(err) => {
            if allow_auth_retry && err.downcast_ref::<SyncUnauthorized>().is_some() {
                // Leave the run status as `Running`: the caller immediately
                // force-refreshes the token and retries, so flashing an error at
                // the user here would be noise for a self-healing condition.
                return AttemptOutcome::Unauthorized;
            }
            eprintln!("sync failed: {err:#}");
            let is_offline = err.downcast_ref::<SyncNetworkUnreachable>().is_some();
            let message = format!("{err:#}");
            // The store queue alone undercounts after a restart, when unpushed
            // edits live only in the persisted sync state — and the poll cadence
            // keys off pending-ness, so count both.
            let disk_pending = load_local_sync_state(&workspace_path())
                .map(|state| state.pending.len())
                .unwrap_or(0);
            let _ = weak.update(cx, |app, cx| {
                let pending_len = app.state.pending_crdt_edits().len().max(disk_pending);
                app.sync_pending_hint = pending_len;
                app.sync_offline = is_offline;
                app.sync_server_rejecting = !is_offline;
                app.sync_run_status = SyncRunStatus::Error {
                    message,
                    pending: pending_len,
                };
                cx.notify();
            });
            AttemptOutcome::Done
        }
    }
}

// Returns Err(()) only when the session is gone (refresh token dead) and the
// caller should abort the sync tick; the user has been signed out. Otherwise
// Ok(()), having either refreshed-and-persisted new tokens or left the current
// (still-valid-enough) ones in place.
async fn ensure_fresh_token(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    force: bool,
) -> Result<(), ()> {
    let account = weak
        .update(cx, |app, _cx| app.settings.sync_account.clone())
        .ok()
        .flatten();
    let Some(account) = account else {
        return Ok(());
    };
    // Refresh even when sync isn't entitled: rotation recomputes entitlement
    // server-side and updates the local `supports_sync` cache, so a subscription
    // purchased on the website reaches a signed-in client within one token
    // lifetime instead of never.
    // Only refresh when the access token is at/near expiry — unless the caller
    // saw the server reject the current token (`force`), which the local expiry
    // clock can't detect.
    if !force
        && account.expires_at > Utc::now() + chrono::Duration::seconds(ACCESS_REFRESH_SKEW_SECS)
    {
        return Ok(());
    }
    let Some(refresh_token) = account.refresh_token.clone() else {
        let _ = weak.update(cx, |app, cx| {
            app.settings.sync_account = None;
            app.sync_auth_status = SyncAuthStatus::Error(
                "Your sync session expired. Please sign in again.".to_string(),
            );
            app.sync_run_status = SyncRunStatus::Idle;
            app.save_app_settings();
            cx.notify();
        });
        return Err(());
    };
    let api_base = account.api_base.clone();
    let outcome = cx
        .background_executor()
        .spawn(async move { refresh_sync_backend(&api_base, &refresh_token) })
        .await;
    match outcome {
        Ok(tokens) => {
            let _ = weak.update(cx, |app, cx| {
                if let Some(acct) = app.settings.sync_account.as_mut() {
                    acct.bearer_token = tokens.bearer_token;
                    acct.expires_at = tokens.expires_at;
                    acct.refresh_token = Some(tokens.refresh_token);
                    acct.refresh_expires_at = tokens.refresh_expires_at;
                    acct.supports_sync = tokens.supports_sync;
                    acct.account_status =
                        Some(SyncAccountStatus::from_supports_sync(tokens.supports_sync));
                }
                app.save_app_settings();
                cx.notify();
            });
            Ok(())
        }
        Err(RefreshError::Unauthorized) => {
            // Refresh token revoked/expired/replayed: the session is gone.
            let _ = weak.update(cx, |app, cx| {
                app.settings.sync_account = None;
                app.sync_auth_status = SyncAuthStatus::Error(
                    "Your sync session expired. Please sign in again.".to_string(),
                );
                app.sync_run_status = SyncRunStatus::Idle;
                app.save_app_settings();
                cx.notify();
            });
            Err(())
        }
        Err(RefreshError::Transient(error)) => {
            // Network/parse hiccup: keep the current token and retry next tick.
            eprintln!("sync token refresh deferred: {error:#}");
            Ok(())
        }
    }
}
