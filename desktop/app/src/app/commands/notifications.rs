use knotq_commands::Command;

use crate::app::KnotQApp;

use super::{
    collect_completion_candidates, collect_deleted_items, item_may_schedule_notifications,
    workspace_item,
};

impl KnotQApp {
    pub(super) fn clear_deleted_item_notifications(&self, cmd: &Command) {
        let mut deleted = Vec::new();
        collect_deleted_items(cmd, &mut deleted);
        deleted.sort_unstable_by_key(|(scheme, item)| (scheme.0, item.0));
        deleted.dedup();
        for (scheme, item) in deleted {
            let Some(item) = workspace_item(&self.workspace, scheme, item).cloned() else {
                continue;
            };
            self.service_bus.signal_clear_item_notifications(
                scheme,
                item,
                self.notification_defaults,
            );
        }
    }

    pub(super) fn clear_completed_occurrence_notifications(&self, cmd: &Command) {
        let mut completed = Vec::new();
        collect_completion_candidates(cmd, &mut completed);
        completed.sort_by_key(|(scheme, item, occurrence)| (scheme.0, item.0, occurrence.clone()));
        completed.dedup();

        for (scheme_id, item_id, occurrence) in completed {
            let Some(item) = workspace_item(&self.workspace, scheme_id, item_id) else {
                continue;
            };
            if !item_may_schedule_notifications(item) {
                continue;
            }
            if !item.state_for_occurrence(&occurrence).is_done() {
                continue;
            }
            self.service_bus.signal_clear_occurrence_notifications(
                scheme_id,
                item.clone(),
                occurrence,
                self.notification_defaults,
            );
        }
    }
}
