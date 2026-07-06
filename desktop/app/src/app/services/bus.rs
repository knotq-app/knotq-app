use knotq_model::{Item, OccurrenceId, SchemeId};
use knotq_storage_json::NotificationDefaults;

use super::{
    AppServiceBus, AppServiceReceivers, NotificationBatch, NotificationItemRefresh,
    NotificationOccurrenceClear, NotificationSignal,
};
use crate::app::sync_service::SyncSignal;

impl AppServiceBus {
    pub(crate) fn new() -> (Self, AppServiceReceivers) {
        let (save_tx, save_rx) = async_channel::bounded(1);
        let (notification_tx, notification_rx) = async_channel::unbounded();
        let (timeline_tx, timeline_rx) = async_channel::bounded(1);
        // Unbounded so Immediate is never dropped when the channel already holds a
        // LocalChange. Signals are drained before each run, so the queue stays tiny
        // in practice (at most a handful of entries between ticks).
        let (sync_tx, sync_rx) = async_channel::unbounded();
        (
            Self {
                save_tx,
                notification_tx,
                timeline_tx,
                sync_tx,
                notification_recompute_pending: std::sync::Arc::new(
                    std::sync::atomic::AtomicBool::new(false),
                ),
                notification_schedule_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                    0,
                )),
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
        self.signal_sync_local_change();
    }

    pub(crate) fn signal_save(&self) {
        let _ = self.save_tx.try_send(());
    }

    fn bump_notification_schedule_gen(&self) {
        self.notification_schedule_gen
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Current notification-schedule generation. Increments whenever a change that
    /// can affect the schedule is signalled; the sync run compares it against the
    /// generation its cached schedule was computed at to decide whether a recompute
    /// is needed.
    pub(crate) fn notification_schedule_generation(&self) -> u64 {
        self.notification_schedule_gen
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn signal_notifications(&self) {
        self.bump_notification_schedule_gen();
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
        scheme_is_daily: bool,
        item: Item,
        defaults: NotificationDefaults,
    ) {
        // A single dated item changed (e.g. its text, so its notification title) —
        // the schedule may differ, so invalidate the sync's cached schedule even
        // when the OS-recompute channel below short-circuits.
        self.bump_notification_schedule_gen();
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
                scheme_is_daily,
                item,
                defaults,
            }));
    }

    pub(crate) fn signal_clear_item_notifications(
        &self,
        scheme_id: SchemeId,
        scheme_is_daily: bool,
        item: Item,
        defaults: NotificationDefaults,
    ) {
        let _ =
            self.notification_tx
                .try_send(NotificationSignal::ClearItem(NotificationItemRefresh {
                    scheme_id,
                    scheme_is_daily,
                    item,
                    defaults,
                }));
    }

    pub(crate) fn signal_clear_occurrence_notifications(
        &self,
        scheme_id: SchemeId,
        scheme_is_daily: bool,
        item: Item,
        occurrence: OccurrenceId,
        defaults: NotificationDefaults,
    ) {
        let _ = self
            .notification_tx
            .try_send(NotificationSignal::ClearOccurrence(
                NotificationOccurrenceClear {
                    scheme_id,
                    scheme_is_daily,
                    item,
                    occurrence,
                    defaults,
                },
            ));
    }

    pub(crate) fn signal_timeline(&self) {
        let _ = self.timeline_tx.try_send(());
    }

    pub(crate) fn signal_sync(&self) {
        let _ = self.sync_tx.try_send(SyncSignal::Immediate);
    }

    /// A cloneable sender for the sync signal channel, so an off-thread callback
    /// (e.g. the WebSocket `changed` nudge) can request an immediate sync run.
    /// Only consumed by the `ws-sync` build today.
    #[cfg_attr(not(feature = "ws-sync"), allow(dead_code))]
    pub(crate) fn sync_signal_sender(&self) -> async_channel::Sender<SyncSignal> {
        self.sync_tx.clone()
    }

    pub(crate) fn signal_sync_local_change(&self) {
        let _ = self.sync_tx.try_send(SyncSignal::LocalChange);
    }

    pub(super) fn signal_notification_action(&self) -> bool {
        self.notification_tx
            .try_send(NotificationSignal::Action)
            .is_ok()
    }

    pub(super) fn clear_notification_recompute_pending(&self) {
        self.notification_recompute_pending
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl NotificationBatch {
    pub(super) fn push(&mut self, signal: NotificationSignal) {
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
            NotificationSignal::ClearOccurrence(clear) => {
                self.occurrence_clears.insert(
                    (clear.scheme_id, clear.item.id, clear.occurrence.clone()),
                    clear,
                );
            }
            NotificationSignal::Action => self.has_actions = true,
        }
    }
}
