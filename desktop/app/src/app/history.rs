use gpui::Context;
use knotq_storage_json::{
    load_workspace_with_options, record_workspace_snapshot, restore_workspace_snapshot,
    save_workspace, workspace_dir, workspace_path, WorkspaceLoadOptions,
};

use super::{daily_queue_initial_start, KnotQApp};

impl KnotQApp {
    pub(crate) fn restore_workspace_to_snapshot(
        &mut self,
        snapshot_id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(reason) = &self.workspace_save_blocked_reason {
            self.workspace_history_error = Some(format!(
                "Cannot restore while workspace load is blocked: {reason}"
            ));
            cx.notify();
            return;
        }

        if let Err(err) = save_workspace(&workspace_path(), &self.workspace) {
            self.workspace_history_error = Some(format!(
                "Could not save current workspace before restore: {err:#}"
            ));
            cx.notify();
            return;
        }
        if let Err(err) = record_workspace_snapshot(&workspace_dir()) {
            eprintln!("pre-restore workspace history snapshot failed: {err:#}");
        }

        if let Err(err) = restore_workspace_snapshot(&workspace_dir(), &snapshot_id) {
            self.workspace_history_error = Some(format!("Could not restore version: {err:#}"));
            cx.notify();
            return;
        }

        self.reload_workspace_after_history_restore(cx);
    }

    fn reload_workspace_after_history_restore(&mut self, cx: &mut Context<Self>) {
        let today = chrono::Local::now().date_naive();
        let loaded_start = daily_queue_initial_start(today);
        let options = WorkspaceLoadOptions::daily_queue_range(loaded_start, today);
        match load_workspace_with_options(&workspace_path(), options) {
            Ok(Some(workspace)) => {
                self.state.replace_workspace(workspace, today, loaded_start);
                self.workspace_save_blocked_reason = None;
                self.workspace_history_error = None;
                self.clear_workspace_ui_after_history_restore();
                self.reconcile_workspace_ui_state();
                self.service_bus.signal_notifications();
                self.service_bus.signal_timeline();
                cx.notify();
            }
            Ok(None) => {
                self.workspace_history_error =
                    Some("Could not restore version: workspace index is missing".to_string());
                cx.notify();
            }
            Err(err) => {
                self.workspace_history_error =
                    Some(format!("Could not reload restored workspace: {err:#}"));
                cx.notify();
            }
        }
    }

    fn clear_workspace_ui_after_history_restore(&mut self) {
        self.undo_navigation_stack.clear();
        self.redo_navigation_stack.clear();
        self.event_popup = None;
        self.event_popup_title_subscription = None;
        self.date_popover = None;
        self.repeat_popover = None;
        self.search_open = false;
        self.search_selected_index = 0;
        self.rename_node = None;
        self.pending_delete = None;
        self.sidebar_context_menu = None;
        self.editor_context_menu = None;
        self.google_calendar_picker = None;
        self.google_calendar_picker_task = None;
        self.scheme_editor = None;
        self._editor_subscription = None;
        self.daily_queue_editors.clear();
        self.daily_queue_editor_subscriptions.clear();
        self.scheme_sessions.clear();
        self.scheme_scroll_initialized_for = None;
        self.daily_queue_scroll_initialized = false;
        self.daily_queue_preserved_bottom_distance = None;
        self.cal_drag = None;
        self.cal_move = None;
        self.cal_resize = None;
    }
}
