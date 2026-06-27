use std::collections::HashMap;

use chrono::Local;
use gpui::{Context, ScrollHandle};
use knotq_state::{daily_queue_default_window_start, AppState};
use knotq_storage_json::{load_crdt_state, load_local_sync_state, workspace_path};

use super::auto_update::{spawn_auto_update_task, AutoUpdateUiStatus};
use super::bootstrap::{load_or_default_settings, load_or_seed};
use super::services::{spawn_notification_task, spawn_save_task, spawn_timeline_task, AppServiceBus};
use super::sync_service::spawn_sync_task;
use super::*;

impl KnotQApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let bootstrap = load_or_seed();
        let workspace = bootstrap.workspace;
        let workspace_save_blocked_reason = bootstrap.save_blocked_reason;
        // Persist IDs generated while loading older scheme files before OS
        // notification actions depend on them.
        let initial_dirty = true;
        let settings = load_or_default_settings();
        let needs_onboarding = !settings.onboarding_completed;
        // Always start with the short tutorial. The sign-in / stay-local prompt is
        // surfaced only after the guide finishes (and skipped entirely if the user
        // is already signed in) — see `render_onboarding`.
        let onboarding_phase = OnboardingPhase::Guide;
        let editor_focus_handle = cx.focus_handle();
        let today = Local::now().date_naive();

        let (service_bus, service_receivers) = AppServiceBus::new();
        let (auto_update_tx, auto_update_rx) = async_channel::bounded(4);
        let save_task = spawn_save_task(service_receivers.save_rx, cx);
        let notification_task =
            spawn_notification_task(service_bus.clone(), service_receivers.notification_rx, cx);
        let state_task = spawn_timeline_task(service_receivers.timeline_rx, cx);
        let sync_task = spawn_sync_task(service_receivers.sync_rx, cx);
        let google_calendar_sync_task = Self::spawn_google_calendar_sync_task(cx);
        let auto_update_task = spawn_auto_update_task(auto_update_rx, cx);
        let quit_subscription = cx.on_app_quit(|app, _cx| {
            app.flush_for_shutdown("app quit");
            async {}
        });
        service_bus.workspace_changed();

        // Restore the long-lived CRDT documents from disk so their stable Yjs
        // identity survives this restart instead of being rebuilt from plain data.
        let crdt_states = load_crdt_state(&workspace_path()).unwrap_or_default();

        // Seed next_sequence from persisted sync state so post-restart edits never
        // reuse sequence numbers still present in the pending queue (Bug 1 fix).
        // Mirrors mobile/core/src/lib.rs:820-841.
        let initial_sequence = {
            let sync_state = load_local_sync_state(&workspace_path()).unwrap_or_default();
            let max_pending = sync_state
                .pending
                .iter()
                .map(|e| e.local_sequence)
                .max()
                .unwrap_or(0);
            let max_pushed = sync_state
                .document_cursors
                .values()
                .map(|c| c.last_pushed_sequence)
                .max()
                .unwrap_or(0);
            max_pending.max(max_pushed) + 1
        };

        let mut app = Self {
            state: AppState::new(
                workspace,
                settings,
                today,
                daily_queue_default_window_start(today),
                initial_dirty,
                crdt_states,
                initial_sequence,
            ),
            settings_return_selection: None,
            event_popup: None,
            event_popup_title_subscription: None,
            date_popover: None,
            repeat_popover: None,
            search_open: false,
            search_input: None,
            search_selected_index: 0,
            editor_focus_handle,
            scheme_editor: None,
            daily_queue_editors: HashMap::new(),
            daily_queue_editor_subscriptions: HashMap::new(),
            scheme_scroll_handle: ScrollHandle::new(),
            scheme_scroll_initialized_for: None,
            scheme_scroll_restore_after_sync: None,
            daily_queue_scroll_handle: ScrollHandle::new(),
            daily_queue_scroll_initialized: false,
            daily_queue_preserved_bottom_distance: None,
            daily_queue_scroll_restore_after_sync: None,
            cal_scroll_handle: ScrollHandle::new(),
            cal_scroll_initialized: false,
            rename_node: None,
            trash_expanded: false,
            pending_delete: None,
            notice_modal: None,
            sidebar_context_menu: None,
            editor_context_menu: None,
            google_calendar_picker: None,
            google_calendar_picker_task: None,
            google_oauth_status: GoogleOAuthStatus::Idle,
            google_oauth_task: None,
            google_oauth_cancel_token: None,
            sync_advance_onboarding_on_success: false,
            sync_auth_status: SyncAuthStatus::Idle,
            sync_auth_cancel_token: None,
            sync_account_action: None,
            settings_dropdown: None,
            sync_run_status: SyncRunStatus::Idle,
            sync_auth_task: None,
            sync_subscription_poll_task: None,
            sync_status_quiet_task: None,
            email_verification_resend: EmailVerificationResend::Idle,
            email_verification_resend_task: None,
            sync_status_popover: None,
            last_synced_at: None,
            last_sync_poll_at: None,
            window_is_active: false,
            sync_offline: false,
            sync_server_rejecting: false,
            sync_pending_hint: 0,
            ws_sync: None,
            ws_sync_token: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            ws_sync_api_base: None,
            scheme_sessions: HashMap::new(),
            service_bus,
            workspace_save_blocked_reason,
            notification_error: None,
            auto_update_status: AutoUpdateUiStatus::initial(),
            auto_update_tx,
            cal_drag: None,
            cal_move: None,
            cal_resize: None,
            cal_swipe: CalendarSwipeState::default(),
            _save_task: save_task,
            _notification_task: notification_task,
            _state_task: state_task,
            _sync_task: sync_task,
            _google_calendar_sync_task: google_calendar_sync_task,
            _auto_update_task: auto_update_task,
            _window_activation_subscription: None,
            _editor_subscription: None,
            _search_subscription: None,
            _appearance_subscription: None,
            _window_bounds_subscription: None,
            _quit_subscription: quit_subscription,
            show_onboarding: needs_onboarding,
            onboarding_phase,
            onboarding_page: 0,
        };
        // Reopen the screen from the last session. Skipped during first-launch
        // onboarding (which drives its own navigation); a saved scheme that was
        // deleted in the meantime falls back to the default Union view.
        if !needs_onboarding {
            app.restore_last_screen(cx);
        }
        app
    }
}
