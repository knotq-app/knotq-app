use gpui::Context;

use crate::app::{KnotQApp, NavSnapshot, View};

use super::{workspace_item, NotificationServiceSignal, WorkspaceServiceSignals};

impl KnotQApp {
    pub(super) fn undo_navigation_snapshot(&self) -> NavSnapshot {
        NavSnapshot {
            selection: self.selection.clone(),
            week_offset: self.week_offset,
            month_offset: self.month_offset,
        }
    }

    pub(super) fn restore_undo_navigation_snapshot(
        &mut self,
        snapshot: &NavSnapshot,
        _cx: &mut Context<Self>,
    ) {
        let view_changed = self.selection.view != snapshot.selection.view
            || self.selection.scheme_id != snapshot.selection.scheme_id;
        if view_changed {
            self.close_date_popover();
            self.close_repeat_popover();
        }

        self.week_offset = snapshot.week_offset;
        self.month_offset = snapshot.month_offset;

        let mut selection = snapshot.selection.clone();
        match selection.view {
            View::Scheme => {
                let Some(scheme_id) = selection.scheme_id else {
                    self.open_union();
                    return;
                };
                let Some(scheme) = self.workspace.scheme(scheme_id) else {
                    self.open_union();
                    return;
                };
                if selection
                    .focused_item_id
                    .is_some_and(|item_id| scheme.item(item_id).is_none())
                {
                    selection.focused_item_id = None;
                }
                self.selection = selection;
            }
            View::DailyQueue => {
                if selection
                    .scheme_id
                    .is_some_and(|scheme_id| self.workspace.scheme(scheme_id).is_none())
                {
                    selection.scheme_id = self
                        .workspace
                        .daily_queue_scheme_id(self.daily_queue_today)
                        .filter(|scheme_id| self.workspace.scheme(*scheme_id).is_some());
                }
                if let Some(scheme_id) = selection.scheme_id {
                    if selection
                        .focused_item_id
                        .is_some_and(|item_id| !self.scheme_item_exists(scheme_id, item_id))
                    {
                        selection.focused_item_id = None;
                    }
                }
                self.selection = selection;
            }
            View::Union | View::Settings => {
                self.selection = selection;
            }
        }

        self.dismiss_event_popup_if_hidden_context();
    }

    pub(super) fn signal_workspace_services(&self, signals: WorkspaceServiceSignals) {
        self.service_bus.signal_save();
        // Every applied command is a local workspace change the sync scheduler
        // should hear about (30 s debounce); without this, edits only reach the
        // server at the next poll tick (up to 30 min in the background).
        self.service_bus.signal_sync_local_change();
        match signals.notifications {
            NotificationServiceSignal::None => {}
            NotificationServiceSignal::Recompute => self.service_bus.signal_notifications(),
            NotificationServiceSignal::RefreshItems(items) => {
                for (scheme_id, item_id) in items {
                    if let Some(item) = workspace_item(&self.workspace, scheme_id, item_id) {
                        self.service_bus.signal_item_notifications(
                            scheme_id,
                            item.clone(),
                            self.notification_defaults,
                        );
                    }
                }
            }
        }
        if signals.timeline {
            self.service_bus.signal_timeline();
        }
    }
}
