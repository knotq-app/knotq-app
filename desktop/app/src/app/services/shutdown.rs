use chrono::Utc;
use knotq_state::AppState;
use knotq_storage_json::{save_crdt_state, save_pending_crdt_edits};

use super::{save_workspace, workspace_path, KnotQApp};

/// The disk half of the shutdown flush: the workspace, then the pending CRDT
/// queue and CRDT document states in lockstep. Only a workspace write failure is
/// returned; the CRDT writes are logged. Shared with the production-path fuzzer.
pub(crate) fn write_shutdown_workspace(
    path: &std::path::Path,
    state: &mut AppState,
) -> anyhow::Result<()> {
    save_workspace(path, &state.workspace)?;
    state.dirty_schemes.clear();
    state.index_dirty = false;
    // Keep the persisted CRDT state in lockstep with the workspace.
    if let Err(err) = save_pending_crdt_edits(path, &state.pending_crdt_edits()) {
        eprintln!("shutdown CRDT pending queue flush failed: {err:#}");
    }
    if let Err(err) = save_crdt_state(path, &state.crdt_document_states()) {
        eprintln!("shutdown CRDT state flush failed: {err:#}");
    }
    Ok(())
}

impl KnotQApp {
    pub(crate) fn flush_for_shutdown(&mut self, reason: &str) {
        crate::notifications::notif_log(&format!("shutdown flush started: {reason}"));

        let completed = knotq_state::complete_past_events(&mut self.state, Utc::now());
        if completed > 0 {
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
            match write_shutdown_workspace(&workspace_path(), &mut self.state) {
                Ok(()) => {
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
