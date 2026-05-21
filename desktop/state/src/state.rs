use std::collections::{HashMap, HashSet, VecDeque};

use chrono::NaiveDate;
use knotq_commands::Command;
use knotq_index::IndexedWorkspace;
use knotq_model::{
    AppSettings, CalendarViewMode, NodeRef, NotificationDefaults, SavedWindowPosition,
    SavedWindowSize, SchemeId, ThemeMode, TimeFormat, Workspace,
};

use crate::{
    DailyQueueState, EditorSessions, EditorUndoGroup, EventBus, NotificationState,
    RetainedCompletedItems, Selection, UndoRedoStack,
};

pub struct AppState {
    pub indexed: IndexedWorkspace,
    pub settings: AppSettings,
    pub dirty_schemes: HashSet<SchemeId>,
    pub index_dirty: bool,
    pub selection: Selection,
    pub week_offset: i32,
    pub month_offset: i32,
    pub undo: UndoRedoStack,
    pub editor_undo_group: Option<EditorUndoGroup>,
    pub recurrence_undo_group: Option<EditorUndoGroup>,
    pub(crate) editor_sessions: EditorSessions,
    pub(crate) retained_completed: RetainedCompletedItems,
    pub(crate) daily_queue: DailyQueueState,
    pub(crate) notifications: NotificationState,
    pub(crate) event_bus: EventBus,

    // Compatibility fields still read directly by knotq-app during the shell
    // slimming phase. Keep them synchronized when dispatching through state.
    pub workspace: Workspace,
    pub theme_mode: ThemeMode,
    pub system_theme_dark: bool,
    pub calendar_view: CalendarViewMode,
    pub time_format: TimeFormat,
    pub notification_defaults: NotificationDefaults,
    pub scheduled_notification_ids: Vec<String>,
    pub daily_queue_today: NaiveDate,
    pub daily_queue_loaded_start: NaiveDate,
    pub daily_queue_visible_dates: HashSet<NaiveDate>,
    pub daily_queue_loaded_calendar_months: HashSet<(i32, u32)>,
    pub retained_completed_calendar_items: HashSet<crate::CalendarOccurrenceKey>,
    pub window_size: Option<SavedWindowSize>,
    pub window_position: Option<SavedWindowPosition>,
    pub undo_stack: VecDeque<Command>,
    pub redo_stack: VecDeque<Command>,
}

impl AppState {
    pub fn new(
        workspace: Workspace,
        settings: AppSettings,
        today: NaiveDate,
        loaded_start: NaiveDate,
        initial_dirty: bool,
    ) -> Self {
        let indexed = IndexedWorkspace::build(workspace.clone());
        let daily_queue = DailyQueueState::new(today, loaded_start);
        let notifications = NotificationState {
            scheduled_ids: settings.scheduled_notification_ids.clone(),
            pending_action_drains: 0,
        };
        let dirty_schemes = if initial_dirty {
            workspace.schemes.keys().copied().collect()
        } else {
            HashSet::new()
        };
        Self {
            indexed,
            settings: settings.clone(),
            dirty_schemes,
            index_dirty: initial_dirty,
            selection: Selection::default(),
            week_offset: 0,
            month_offset: 0,
            undo: UndoRedoStack::default(),
            editor_undo_group: None,
            recurrence_undo_group: None,
            editor_sessions: HashMap::new(),
            retained_completed: RetainedCompletedItems::default(),
            daily_queue: daily_queue.clone(),
            notifications,
            event_bus: EventBus::default(),
            workspace,
            theme_mode: settings.theme_mode,
            system_theme_dark: true,
            calendar_view: settings.calendar_view,
            time_format: settings.time_format,
            notification_defaults: settings.notification_defaults,
            scheduled_notification_ids: settings.scheduled_notification_ids,
            daily_queue_today: today,
            daily_queue_loaded_start: loaded_start,
            daily_queue_visible_dates: daily_queue.visible_dates,
            daily_queue_loaded_calendar_months: daily_queue.loaded_calendar_months,
            retained_completed_calendar_items: HashSet::new(),
            window_size: settings.window_size,
            window_position: settings.window_position,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
        }
    }

    pub fn subscribe(&mut self) -> std::sync::mpsc::Receiver<crate::AppEvent> {
        self.event_bus.subscribe()
    }

    pub fn editor_session_mut(&mut self, scheme_id: SchemeId) -> &mut crate::EditorSession {
        self.editor_sessions.entry(scheme_id).or_default()
    }

    pub fn daily_queue_state(&self) -> &DailyQueueState {
        &self.daily_queue
    }

    pub fn notification_state(&self) -> &NotificationState {
        &self.notifications
    }

    pub fn retained_completed(&self) -> &RetainedCompletedItems {
        &self.retained_completed
    }

    pub fn select_node(&mut self, target: NodeRef) {
        if let NodeRef::Scheme(scheme_id) = target {
            self.selection.scheme_id = Some(scheme_id);
            self.selection.view = crate::View::Scheme;
        }
    }

    /// Mark the workspace as dirty due to a command. Tracks which schemes were
    /// affected so only their files need to be written.
    pub fn mark_dirty_from_command(&mut self, cmd: &Command) {
        self.index_dirty = true;
        collect_affected_schemes(cmd, &mut self.dirty_schemes);
    }

    /// Mark a single scheme as dirty.
    pub fn mark_scheme_dirty(&mut self, scheme_id: SchemeId) {
        self.dirty_schemes.insert(scheme_id);
        self.index_dirty = true;
    }

    /// Mark only the workspace index as dirty (folder structure changes, etc.)
    pub fn mark_index_dirty(&mut self) {
        self.index_dirty = true;
    }

    /// Returns true if any scheme or the index needs saving.
    pub fn is_dirty(&self) -> bool {
        self.index_dirty || !self.dirty_schemes.is_empty()
    }

    pub fn replace_workspace(
        &mut self,
        workspace: Workspace,
        today: NaiveDate,
        loaded_start: NaiveDate,
    ) {
        self.indexed = IndexedWorkspace::build(workspace.clone());
        self.workspace = workspace;
        self.dirty_schemes.clear();
        self.index_dirty = false;
        self.undo = UndoRedoStack::default();
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        self.editor_sessions.clear();
        self.retained_completed_calendar_items.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();

        let daily_queue = DailyQueueState::new(today, loaded_start);
        self.daily_queue = daily_queue.clone();
        self.daily_queue_today = today;
        self.daily_queue_loaded_start = loaded_start;
        self.daily_queue_visible_dates = daily_queue.visible_dates;
        self.daily_queue_loaded_calendar_months = daily_queue.loaded_calendar_months;
    }
}

fn collect_affected_schemes(cmd: &Command, out: &mut HashSet<SchemeId>) {
    match cmd {
        Command::InsertItem { scheme, .. }
        | Command::UpdateItemText { scheme, .. }
        | Command::ReplaceItem { scheme, .. }
        | Command::SetItemIndent { scheme, .. }
        | Command::SetItemMarker { scheme, .. }
        | Command::SetItemDate { scheme, .. }
        | Command::SetItemRecurrence { scheme, .. }
        | Command::SetItemPriority { scheme, .. }
        | Command::SetOccurrenceNotificationOffset { scheme, .. }
        | Command::ToggleOccurrence { scheme, .. }
        | Command::DeleteItem { scheme, .. }
        | Command::ReorderItem { scheme, .. }
        | Command::RenameScheme { id: scheme, .. }
        | Command::SetSchemeColor { id: scheme, .. }
        | Command::SetSchemeGsync { id: scheme, .. }
        | Command::SetSchemeSource { id: scheme, .. }
        | Command::DeleteScheme { id: scheme }
        | Command::PermanentlyDeleteScheme { id: scheme } => {
            out.insert(*scheme);
        }
        Command::RestoreScheme { scheme, .. } | Command::RestoreDeletedScheme { scheme, .. } => {
            out.insert(scheme.id);
        }
        Command::Batch(cmds) => {
            for cmd in cmds {
                collect_affected_schemes(cmd, out);
            }
        }
        // Folder operations only affect the index.
        Command::CreateFolder { .. }
        | Command::RestoreFolder { .. }
        | Command::RenameFolder { .. }
        | Command::SetFolderExpanded { .. }
        | Command::DeleteFolder { .. }
        | Command::CreateScheme { .. }
        | Command::MoveNode { .. } => {}
    }
}
