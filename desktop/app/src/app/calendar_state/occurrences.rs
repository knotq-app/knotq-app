use chrono::{DateTime, Utc};
use gpui::Context;
use knotq_commands::Command;
use knotq_model::{ItemId, OccurrenceId, SchemeId};

use crate::app::{CalendarOccurrenceKey, KnotQApp};
use knotq_state::mark_past_event_completion_keys_done;

impl KnotQApp {
    pub fn toggle_calendar_item(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        occurrence: OccurrenceId,
        cx: &mut Context<Self>,
    ) {
        // Completion is local-only state, so it's allowed even on a read-only
        // (imported) scheme — only the occurrence-shape check below can block it.
        if !self.item_allows_occurrence_toggle(scheme_id, item_id, &occurrence) {
            return;
        }
        self.apply(
            Command::ToggleOccurrence {
                scheme: scheme_id,
                item: item_id,
                occurrence,
            },
            cx,
        );
    }

    pub(crate) fn complete_past_event_occurrences(
        &mut self,
        keys: &[CalendarOccurrenceKey],
        now: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> usize {
        let changed = mark_past_event_completion_keys_done(&mut self.workspace, keys, now);
        if changed == 0 {
            return 0;
        }
        for key in keys {
            let Some(item) = self
                .workspace
                .scheme(key.scheme_id)
                .and_then(|scheme| scheme.item(key.item_id))
            else {
                continue;
            };
            if !item.state_for_occurrence(&key.occurrence).is_done() {
                continue;
            }
            self.service_bus.signal_clear_occurrence_notifications(
                key.scheme_id,
                item.clone(),
                key.occurrence.clone(),
                self.notification_defaults,
            );
        }
        // Completion keys can come from any scheme in the background snapshot.
        let all_ids: Vec<_> = self.workspace.schemes.keys().copied().collect();
        for id in all_ids {
            self.dirty_schemes.insert(id);
        }
        self.index_dirty = true;
        self.state.mark_direct_workspace_dirty();
        self.reconcile_workspace_ui_state();
        self.reschedule_notifications();
        cx.notify();
        changed
    }

    pub(crate) fn reschedule_notifications(&mut self) {
        self.service_bus.signal_notifications();
        self.service_bus.signal_timeline();
        self.service_bus.signal_save();
    }
}
