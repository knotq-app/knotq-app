//! Top-level application state and root entity.
//!
//! Holds a single [`Workspace`] (the source of truth) plus ephemeral UI state
//! (selection, theme, week offset, popovers). All workspace mutations go
//! through `apply_command`, which updates the model, marks dirty for save,
//! and notifies the entity.
//!
//! Larger logical groups are split into submodules:
//! - `settings`      – theme, calendar view, time format, window bounds
//! - `nav`           – navigation between views and calendar period
//! - `commands`      – apply / undo / redo
//! - `workspace_ops` – scheme/folder CRUD and UI state reconciliation
//! - `editor_mgr`    – scheme editor lifecycle and event handling
//! - `node_rename`   – inline rename and new-node prompt
//! - `calendar_state`– calendar toggle state and event completion

pub(crate) mod auto_update;
mod bootstrap;
mod calendar_state;
mod commands;
mod constructor;
mod daily_queue;
mod delete_confirm;
mod editor_mgr;
mod google_oauth;
mod nav;
mod node_rename;
mod services;
mod settings;
mod sync_auth;
mod sync_service;
mod workspace_ops;

// Re-export public initialization helpers used by main.rs.
pub use bootstrap::load_or_default_settings;
pub use settings::initial_window_bounds;

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{atomic::AtomicBool, Arc};

use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use gpui::{Context, Entity, FocusHandle, Pixels, Point, ScrollHandle, Subscription, Task};
use gpui_component::input::InputState;
use knotq_commands::DateKind;
use knotq_model::{
    FolderId, Item, ItemId, ItemState, NodeRef, OccurrenceId, Recurrence, Scheme, SchemeId,
    Workspace,
};
pub use knotq_state::{
    add_months, calendar_month_keys_between, calendar_toggle_keys, daily_queue_carryover_command,
    daily_queue_default_window_start, daily_queue_initial_start, daily_queue_scheme_is_blank,
    daily_queue_scheme_name, editor_undo_key, last_nonempty_daily_queue_day,
    make_default_workspace_for_date, recurrence_undo_key, should_coalesce_editor_undo,
    should_coalesce_recurrence_undo, AppState, CalendarOccurrenceKey, EditorUndoGroup,
    EditorUndoKey, NavSnapshot, Selection, UndoEntry, View, DAILY_QUEUE_DEFAULT_WINDOW_DAYS,
};
use knotq_storage_json::{
    load_app_settings, load_daily_queue_scheme, load_daily_queue_schemes_for_calendar_range,
    load_workspace_with_options, save_workspace, save_workspace_incremental, settings_path,
    workspace_path, AppSettings, WorkspaceLoadOptions,
};

use auto_update::{AutoUpdateSignal, AutoUpdateUiStatus};
use services::AppServiceBus;

use knotq_editor::{SchemeEditor, SchemeEditorSessionState, TableContext};
use knotq_ui::date_field::DateComponentField;
use knotq_ui::single_line_editor::SingleLineEditor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepeatScope {
    AllEvents,
    ThisEvent,
    AllFuture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventScopeAction {
    ApplyChanges,
    Delete,
}

// ── Overlay / popup state ─────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EventPopup {
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub title_input: Option<Entity<SingleLineEditor>>,
    pub draft_title: String,
    pub created_from_calendar: bool,
    pub occurrence: OccurrenceId,
    pub draft_start: Option<DateTime<Utc>>,
    pub draft_end: Option<DateTime<Utc>>,
    pub draft_repeats: Option<Recurrence>,
    pub draft_notification_offset_secs: Option<i64>,
    pub draft_done: bool,
    pub start_dirty: bool,
    pub end_dirty: bool,
    pub repeats_dirty: bool,
    pub notification_dirty: bool,
    pub done_dirty: bool,
    pub title_dirty: bool,
    pub anchor: gpui::Point<gpui::Pixels>,
    pub notification_menu_open: bool,
    pub repeat_menu_open: bool,
    pub scope_action: Option<EventScopeAction>,
    pub scope_dialog_only: bool,
    pub scheme_menu_open: bool,
    pub until_picker_open: bool,
    pub until_calendar_anchor_y: gpui::Pixels,
    pub until_display_month: Option<NaiveDate>,
    pub occurrence_index: usize,
}

impl EventPopup {
    pub fn close_all_menus(&mut self) {
        self.notification_menu_open = false;
        self.repeat_menu_open = false;
        self.until_picker_open = false;
        self.scheme_menu_open = false;
    }

    pub fn new(
        scheme_id: SchemeId,
        item_id: ItemId,
        item: &Item,
        occurrence: OccurrenceId,
        occurrence_state: &ItemState,
        draft_start: Option<DateTime<Utc>>,
        draft_end: Option<DateTime<Utc>>,
        anchor: gpui::Point<gpui::Pixels>,
        occurrence_index: usize,
    ) -> Self {
        Self {
            scheme_id,
            item_id,
            title_input: None,
            draft_title: item.text(),
            created_from_calendar: false,
            occurrence,
            draft_start,
            draft_end,
            draft_repeats: item.repeats.clone(),
            draft_notification_offset_secs: occurrence_state.notification_offset_secs,
            draft_done: occurrence_state.is_done(),
            start_dirty: false,
            end_dirty: false,
            repeats_dirty: false,
            notification_dirty: false,
            done_dirty: false,
            title_dirty: false,
            anchor,
            notification_menu_open: false,
            repeat_menu_open: false,
            scope_action: None,
            scope_dialog_only: false,
            scheme_menu_open: false,
            until_picker_open: false,
            until_calendar_anchor_y: gpui::px(0.0),
            until_display_month: None,
            occurrence_index,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeleteConfirmation {
    pub target: ConfirmationTarget,
    pub title: String,
    pub message: String,
    pub confirm_label: String,
}

#[derive(Clone, Debug)]
pub enum ConfirmationTarget {
    EmptyArchive,
    GoogleAccount { account_id: String },
}

#[derive(Clone, Debug)]
pub struct NoticeModal {
    pub title: String,
    pub message: String,
    pub button_label: String,
}

#[derive(Clone, Debug)]
pub enum SidebarContextTarget {
    Background,
    NewMenu { parent: FolderId },
    GoogleCalendarPicker { parent: FolderId },
    Archive,
    Folder(FolderId),
    Scheme { scheme_id: SchemeId },
    DeletedScheme { scheme_id: SchemeId },
    DeletedFolder { folder_id: FolderId },
}

#[derive(Clone, Debug)]
pub struct GoogleCalendarPickerState {
    pub parent: FolderId,
    pub status: GoogleCalendarPickerStatus,
}

#[derive(Clone, Debug)]
pub enum GoogleCalendarPickerStatus {
    Loading,
    Loaded {
        accounts: Vec<GoogleCalendarPickerAccount>,
    },
    Error(String),
}

#[derive(Clone, Debug)]
pub struct GoogleCalendarPickerAccount {
    pub account_id: String,
    pub label: String,
    pub calendars: Vec<GoogleCalendarPickerCalendar>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GoogleCalendarPickerCalendar {
    pub id: String,
    pub label: String,
    pub already_added: bool,
}

#[derive(Clone, Debug, Default)]
pub enum GoogleOAuthStatus {
    #[default]
    Idle,
    InProgress,
    Error,
}

#[derive(Clone, Debug, Default)]
pub enum SyncAuthStatus {
    #[default]
    Idle,
    InProgress,
    Error(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncAuthMode {
    SignIn,
    CreateAccount,
}

/// Tracks a resend of the email-verification link, so the UI can show progress and
/// a one-shot confirmation without re-arming on every render. Reset to `Idle` when
/// the account status is refreshed (which is also how a now-verified account stops
/// showing the prompt).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum EmailVerificationResend {
    #[default]
    Idle,
    InProgress,
    Sent,
}

/// An account action awaiting an explicit second confirmation in Settings, so a
/// single misclick cannot change billing state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncAccountAction {
    /// Turn off the sync entitlement for the account (keeps the account + data).
    CancelSubscription,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsDropdown {
    Theme,
    CalendarView,
    CalendarRange,
    TimeFormat,
    EventNotification,
    AssignmentNotification,
    SyncAccountManage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnboardingPhase {
    AccountChoice,
    Guide,
}

#[derive(Clone, Debug, Default)]
pub enum SyncRunStatus {
    #[default]
    Idle,
    Running {
        pending: usize,
    },
    Synced {
        pending: usize,
    },
    Error {
        message: String,
        pending: usize,
    },
}

#[derive(Clone, Debug)]
pub struct SidebarContextMenu {
    pub target: SidebarContextTarget,
    pub position: gpui::Point<gpui::Pixels>,
}

#[derive(Clone, Debug)]
pub struct EditorContextMenu {
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub position: gpui::Point<gpui::Pixels>,
    pub date_anchor: gpui::Point<gpui::Pixels>,
    pub table: Option<TableContext>,
}

pub struct DatePickerPopover {
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub kind: DateKind,
    pub anchor: gpui::Point<gpui::Pixels>,
    pub hour_is_pm: bool,
    pub year_input: Entity<DateComponentField>,
    pub month_input: Entity<DateComponentField>,
    pub day_input: Entity<DateComponentField>,
    pub hour_input: Entity<DateComponentField>,
    pub minute_input: Entity<DateComponentField>,
    pub _year_subscription: Subscription,
    pub _month_subscription: Subscription,
    pub _day_subscription: Subscription,
    pub _hour_subscription: Subscription,
    pub _minute_subscription: Subscription,
}

pub struct RepeatPopover {
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub anchor: gpui::Point<gpui::Pixels>,
    pub occurrence_index: Option<usize>,
    pub scope: RepeatScope,
    pub type_menu_open: bool,
    pub end_menu_open: bool,
    pub until_open: bool,
    pub until_display_month: Option<NaiveDate>,
}

// ── Sidebar/rename state ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewNodeKind {
    Folder,
    Scheme,
}

pub struct RenameNodeState {
    pub target: NodeRef,
    pub original_name: String,
    pub input: Entity<SingleLineEditor>,
    pub error: Option<String>,
    pub _subscription: Subscription,
}

// ── Editor session state ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) struct SchemeSessionState {
    pub(crate) editor: SchemeEditorSessionState,
    pub(crate) scroll_offset: Point<Pixels>,
    pub(crate) menu: Option<SchemeEditorMenuState>,
}

#[derive(Clone, Debug)]
pub(crate) enum SchemeEditorMenuState {
    Date {
        item_id: ItemId,
        kind: DateKind,
        anchor: Point<Pixels>,
    },
    Repeat {
        item_id: ItemId,
        anchor: Point<Pixels>,
    },
    Context {
        item_id: ItemId,
        position: Point<Pixels>,
        date_anchor: Point<Pixels>,
        table: Option<TableContext>,
    },
}

// ── Calendar drag state ──────────────────────────────────────────────────

/// Tracks an in-progress mouse drag on the calendar week view.
/// Used to create new events/assignments/reminders by clicking or dragging.
#[derive(Clone, Debug)]
pub struct CalendarDragState {
    /// Column date where the drag started.
    pub date: NaiveDate,
    /// Hour fraction (0.0–24.0) where the drag started.
    pub start_hour: f32,
    /// Current hour fraction — updated on mouse move.
    pub current_hour: f32,
    /// Whether pointer movement has crossed the create-drag threshold.
    pub is_dragging: bool,
    /// Whether shift was held at the start (shift+click = assignment).
    pub shift: bool,
}

/// Tracks dragging an existing calendar item to reschedule it.
#[derive(Clone, Debug)]
pub struct CalendarMoveState {
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub occurrence: OccurrenceId,
    pub occurrence_index: usize,
    /// Date column where the item lives.
    pub date: NaiveDate,
    /// Date column where the drag started.
    pub original_date: NaiveDate,
    /// Materialized start/end for this rendered occurrence.
    pub occurrence_start: Option<DateTime<Utc>>,
    pub occurrence_end: Option<DateTime<Utc>>,
    /// Hour where the drag began.
    pub grab_hour: f32,
    /// Window X (px) where the drag began. The target day is derived from the
    /// horizontal displacement from this point, so small sideways wobble during
    /// a mostly-vertical drag does not flip the day.
    pub grab_x: f32,
    /// Current hour offset from grab point.
    pub current_hour: f32,
    /// Last pointer position, used to anchor recurrence-scope confirmation.
    pub anchor: Point<Pixels>,
}

impl CalendarMoveState {
    /// Vertical drag snapped to the 15-minute grid, as a minute offset. This is
    /// the single source of truth used by both the drag ghost and the commit so
    /// they can never disagree.
    pub fn snapped_minute_delta(&self) -> i64 {
        let minutes = ((self.current_hour - self.grab_hour) * 60.0).round() as i64;
        ((minutes as f64 / 15.0).round() as i64) * 15
    }

    /// Whole-day offset from where the drag began.
    pub fn day_delta(&self) -> i64 {
        self.date
            .signed_duration_since(self.original_date)
            .num_days()
    }

    /// A move with no day change and no snapped time change — treated as a click.
    pub fn is_negligible(&self) -> bool {
        self.day_delta() == 0 && self.snapped_minute_delta() == 0
    }

    /// The start/end this move resolves to. The ghost and the commit both derive
    /// their position from this, so what you see while dragging is exactly where
    /// the item lands on release.
    pub fn draft_dates(&self) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        let delta =
            Duration::days(self.day_delta()) + Duration::minutes(self.snapped_minute_delta());
        (
            self.occurrence_start.map(|start| start + delta),
            self.occurrence_end.map(|end| end + delta),
        )
    }
}

/// Tracks dragging the bottom edge of an event to resize its end time.
#[derive(Clone, Debug)]
pub struct CalendarResizeState {
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub occurrence: OccurrenceId,
    pub occurrence_index: usize,
    /// Date column where the resize started.
    pub date: NaiveDate,
    /// Materialized start/end for this rendered occurrence.
    pub occurrence_start: DateTime<Utc>,
    pub occurrence_end: DateTime<Utc>,
    /// Original start hour within `date`.
    pub original_start_hour: f32,
    /// Current bottom edge hour.
    pub current_hour: f32,
    /// Last pointer position, used to anchor recurrence-scope confirmation.
    pub anchor: Point<Pixels>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CalendarSwipeState {
    pub offset_x: f32,
}

// ── Constants ─────────────────────────────────────────────────────────────

pub const CALENDAR_WEEK_VIEW_DAYS: usize = 7;
pub const DAILY_QUEUE_TITLE: &str = "Daily";
pub const DAILY_QUEUE_MARKER_COLOR_DARK: u32 = 0xb8c9e8ff;
pub const DAILY_QUEUE_MARKER_COLOR_LIGHT: u32 = 0x5a7aadff;

pub fn daily_queue_marker_color(is_dark: bool) -> u32 {
    if is_dark {
        DAILY_QUEUE_MARKER_COLOR_DARK
    } else {
        DAILY_QUEUE_MARKER_COLOR_LIGHT
    }
}
pub const DAILY_QUEUE_COLOR_INDEX: u8 = 0;
pub const DEFAULT_WINDOW_WIDTH: f32 = 1250.0;
pub const DEFAULT_WINDOW_HEIGHT: f32 = 750.0;
pub const MIN_WINDOW_WIDTH: f32 = 800.0;
// Page older days in two-week chunks (matching the initial render window) so
// each scroll-back expansion only materializes a couple weeks of editors at a
// time rather than a whole month in a single frame.
pub(super) const DAILY_QUEUE_PAGE_DAYS: i64 = DAILY_QUEUE_DEFAULT_WINDOW_DAYS;

// ── KnotQApp struct ────────────────────────────────────────────────────────

pub struct KnotQApp {
    pub state: AppState,
    pub(super) settings_return_selection: Option<Selection>,
    pub event_popup: Option<EventPopup>,
    pub(crate) event_popup_title_subscription: Option<Subscription>,
    pub date_popover: Option<DatePickerPopover>,
    pub repeat_popover: Option<RepeatPopover>,
    pub search_open: bool,
    pub search_input: Option<Entity<InputState>>,
    pub search_selected_index: usize,
    pub editor_focus_handle: FocusHandle,
    pub scheme_editor: Option<(SchemeId, Entity<SchemeEditor>)>,
    pub daily_queue_editors: HashMap<NaiveDate, Entity<SchemeEditor>>,
    pub(crate) daily_queue_editor_subscriptions: HashMap<NaiveDate, Subscription>,
    pub scheme_scroll_handle: ScrollHandle,
    pub scheme_scroll_initialized_for: Option<SchemeId>,
    pub(crate) scheme_scroll_restore_after_sync: Option<(SchemeId, Point<Pixels>)>,
    pub daily_queue_scroll_handle: ScrollHandle,
    pub daily_queue_scroll_initialized: bool,
    pub daily_queue_preserved_bottom_distance: Option<Pixels>,
    pub(crate) daily_queue_scroll_restore_after_sync: Option<Point<Pixels>>,
    pub cal_scroll_handle: ScrollHandle,
    pub cal_scroll_initialized: bool,
    pub rename_node: Option<RenameNodeState>,
    pub trash_expanded: bool,
    pub pending_delete: Option<DeleteConfirmation>,
    pub notice_modal: Option<NoticeModal>,
    pub sidebar_context_menu: Option<SidebarContextMenu>,
    pub editor_context_menu: Option<EditorContextMenu>,
    pub google_calendar_picker: Option<GoogleCalendarPickerState>,
    pub google_calendar_picker_task: Option<Task<()>>,
    pub google_oauth_status: GoogleOAuthStatus,
    pub google_oauth_task: Option<Task<()>>,
    pub google_oauth_cancel_token: Option<Arc<AtomicBool>>,
    /// Sign-in now happens in the browser; this just remembers whether the
    /// in-flight browser sign-in should advance onboarding when it succeeds.
    pub sync_advance_onboarding_on_success: bool,
    pub sync_auth_status: SyncAuthStatus,
    pub sync_run_status: SyncRunStatus,
    pub sync_auth_task: Option<Task<()>>,
    /// Cancel signal for the in-flight browser sign-in, so re-clicking "Sign in"
    /// during the polling window can abort the old loopback wait and relaunch the
    /// browser (mirroring the Google Calendar OAuth flow).
    pub sync_auth_cancel_token: Option<Arc<AtomicBool>>,
    /// Bounded background poll that re-checks entitlement after the user opens the
    /// subscription checkout, so sync turns on without them clicking anything.
    pub sync_subscription_poll_task: Option<Task<()>>,
    /// One-shot background account-status refresh fired when Settings opens, so
    /// an entitlement change made outside the app shows up without any clicks.
    pub sync_status_quiet_task: Option<Task<()>>,
    /// State of a resend of the email-verification link (shown in the Sync card).
    pub email_verification_resend: EmailVerificationResend,
    /// In-flight resend task; kept alive so it isn't dropped mid-request.
    pub email_verification_resend_task: Option<Task<()>>,
    /// The currently expanded compact selector in Settings.
    pub settings_dropdown: Option<SettingsDropdown>,
    /// Pending confirmation for a destructive account action shown in Settings.
    pub sync_account_action: Option<SyncAccountAction>,
    /// Anchor for the title-bar sync status popover; `Some` while it is open.
    pub sync_status_popover: Option<Point<Pixels>>,
    /// When the last sync completed successfully, for the "Last synced …" line.
    pub last_synced_at: Option<DateTime<Utc>>,
    /// Last attempt to run the sync loop.
    pub last_sync_poll_at: Option<DateTime<Utc>>,
    /// Whether the application window is active (receiving input / key focus).
    pub window_is_active: bool,
    /// Whether the last sync run failed at the transport level (offline).
    pub sync_offline: bool,
    /// Whether the last sync run failed with a server rejection (non-transport error).
    pub sync_server_rejecting: bool,
    /// Remaining pending count from the last sync run result; persisted so the
    /// poll-interval logic can see pending edits even between runs.
    pub sync_pending_hint: usize,
    pub(crate) scheme_sessions: HashMap<SchemeId, SchemeSessionState>,
    pub(crate) service_bus: AppServiceBus,
    pub(crate) workspace_save_blocked_reason: Option<String>,
    pub notification_error: Option<String>,
    pub auto_update_status: AutoUpdateUiStatus,
    pub(crate) auto_update_tx: async_channel::Sender<AutoUpdateSignal>,
    pub cal_drag: Option<CalendarDragState>,
    pub cal_move: Option<CalendarMoveState>,
    pub cal_resize: Option<CalendarResizeState>,
    pub cal_swipe: CalendarSwipeState,
    pub _save_task: Task<()>,
    pub _notification_task: Task<()>,
    pub _state_task: Task<()>,
    pub _sync_task: Task<()>,
    pub _google_calendar_sync_task: Task<()>,
    pub _auto_update_task: Task<()>,
    pub _window_activation_subscription: Option<Subscription>,
    pub _editor_subscription: Option<Subscription>,
    pub _search_subscription: Option<Subscription>,
    pub _appearance_subscription: Option<Subscription>,
    pub _window_bounds_subscription: Option<Subscription>,
    pub _quit_subscription: Subscription,
    pub show_onboarding: bool,
    pub onboarding_phase: OnboardingPhase,
    pub onboarding_page: usize,
}

impl Deref for KnotQApp {
    type Target = AppState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for KnotQApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

// ── Constructor ───────────────────────────────────────────────────────────
//
// `impl KnotQApp { fn new }` lives in the `constructor` submodule.
