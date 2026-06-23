use chrono::Utc;
use knotq_storage_json::save_crdt_state;

use super::{save_workspace, workspace_path, KnotQApp};

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
            self.state.mark_direct_workspace_dirty();
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
                    // Keep the persisted CRDT state in lockstep with the workspace.
                    if let Err(err) =
                        save_crdt_state(&workspace_path(), &self.state.crdt_document_states())
                    {
                        eprintln!("shutdown CRDT state flush failed: {err:#}");
                    }
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
        let completed_cleanup_error = crate::notifications::clear_completed_notifications(
            &self.workspace,
            self.notification_defaults,
            Utc::now(),
        );
        let cleanup_error = crate::notifications::clear_expired_event_notifications(
            &self.workspace,
            self.notification_defaults,
            Utc::now(),
        );
        if let Some(err) = schedule_error.or(completed_cleanup_error).or(cleanup_error) {
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
