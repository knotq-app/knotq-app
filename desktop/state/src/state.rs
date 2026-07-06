use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use knotq_commands::{Command, CommandError, CommandOrigin, CommandReceipt};
use knotq_index::IndexedWorkspace;
use knotq_model::{
    AppSettings, CalendarViewMode, CalendarWeekRange, DocumentId, NodeRef, NotificationDefaults,
    SavedWindowPosition, SavedWindowSize, SchemeId, ThemeMode, TimeFormat, Workspace,
};
use knotq_sync::PendingCrdtEdit;

use crate::{
    DailyQueueState, EditorSessions, EditorUndoGroup, EventBus, NotificationState,
    RetainedCompletedItems, Selection, UndoScope, UndoStore, View, WorkspaceDirtyState,
    WorkspaceStore,
};

pub struct AppState {
    pub(crate) store: WorkspaceStore,
    pub settings: AppSettings,
    pub dirty_schemes: HashSet<SchemeId>,
    pub index_dirty: bool,
    pub selection: Selection,
    pub week_offset: i32,
    pub month_offset: i32,
    pub undo_store: UndoStore,
    pub editor_undo_group: Option<EditorUndoGroup>,
    pub recurrence_undo_group: Option<EditorUndoGroup>,
    pub(crate) editor_sessions: EditorSessions,
    pub(crate) retained_completed: RetainedCompletedItems,
    pub(crate) daily_queue: DailyQueueState,
    pub(crate) notifications: NotificationState,
    pub(crate) event_bus: EventBus,
    // True when app code has mutated `workspace` directly and the canonical store
    // must be rebuilt before the next dispatched command.
    direct_workspace_dirty: bool,

    // Fields still read directly by knotq-app during the shell slimming phase.
    // Keep them synchronized when dispatching through state.
    pub workspace: Workspace,
    pub theme_mode: ThemeMode,
    pub system_theme_dark: bool,
    pub calendar_view: CalendarViewMode,
    pub calendar_week_range: CalendarWeekRange,
    pub time_format: TimeFormat,
    pub notification_defaults: NotificationDefaults,
    pub scheduled_notification_ids: Vec<String>,
    pub daily_queue_today: NaiveDate,
    pub daily_queue_loaded_start: NaiveDate,
    pub daily_queue_visible_dates: HashSet<NaiveDate>,
    pub daily_queue_loaded_calendar_months: HashSet<(i32, u32)>,
    pub window_size: Option<SavedWindowSize>,
    pub window_position: Option<SavedWindowPosition>,
}

impl AppState {
    pub fn new(
        workspace: Workspace,
        settings: AppSettings,
        today: NaiveDate,
        loaded_start: NaiveDate,
        initial_dirty: bool,
        crdt_states: HashMap<DocumentId, Vec<u8>>,
        initial_sequence: u64,
    ) -> Self {
        let store = WorkspaceStore::new(
            workspace,
            settings.replica_id,
            initial_dirty,
            crdt_states,
            initial_sequence,
        );
        let daily_queue = DailyQueueState::new(today, loaded_start);
        let notifications = NotificationState {
            scheduled_ids: settings.scheduled_notification_ids.clone(),
            pending_action_drains: 0,
        };
        let workspace = store.workspace().clone();
        let dirty_schemes = store.dirty().schemes.clone();
        Self {
            store,
            settings: settings.clone(),
            dirty_schemes,
            index_dirty: initial_dirty,
            selection: Selection::default(),
            week_offset: 0,
            month_offset: 0,
            undo_store: UndoStore::default(),
            editor_undo_group: None,
            recurrence_undo_group: None,
            editor_sessions: HashMap::new(),
            retained_completed: RetainedCompletedItems::default(),
            daily_queue: daily_queue.clone(),
            notifications,
            event_bus: EventBus::default(),
            direct_workspace_dirty: false,
            workspace,
            theme_mode: settings.theme_mode,
            system_theme_dark: true,
            calendar_view: settings.calendar_view,
            calendar_week_range: settings.calendar_week_range,
            time_format: settings.time_format,
            notification_defaults: settings.notification_defaults,
            scheduled_notification_ids: settings.scheduled_notification_ids,
            daily_queue_today: today,
            daily_queue_loaded_start: loaded_start,
            daily_queue_visible_dates: daily_queue.visible_dates,
            daily_queue_loaded_calendar_months: daily_queue.loaded_calendar_months,
            window_size: settings.window_size,
            window_position: settings.window_position,
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

    pub fn retained_completed_mut(&mut self) -> &mut RetainedCompletedItems {
        &mut self.retained_completed
    }

    pub fn select_node(&mut self, target: NodeRef) {
        if let NodeRef::Scheme(scheme_id) = target {
            self.selection.scheme_id = Some(scheme_id);
            self.selection.view = crate::View::Scheme;
        }
    }

    /// The undo timeline a plain undo/redo keypress targets, derived from the
    /// current view: a focused scheme undoes its own content edits; views with
    /// no focused scheme (the calendar) fall back to the global timeline.
    pub fn active_undo_scope(&self) -> UndoScope {
        match self.selection.view {
            View::Scheme | View::DailyQueue => match self.selection.scheme_id {
                Some(scheme) => UndoScope::Scheme(scheme),
                None => UndoScope::Workspace,
            },
            View::Union | View::Settings => UndoScope::Workspace,
        }
    }

    /// The timeline a freshly applied `command` should file its undo entry
    /// under, given where it was initiated (the current view). Lets a calendar
    /// action that edits a per-scheme item still undo from the calendar.
    pub fn undo_scope_for(&self, command: &Command) -> UndoScope {
        UndoScope::for_command(command, self.active_undo_scope())
    }

    /// Mark the workspace as dirty due to a command. Tracks which schemes were
    /// affected so only their files need to be written.
    pub fn mark_dirty_from_command(&mut self, cmd: &Command) {
        self.store.mark_dirty_from_command(cmd);
        self.sync_workspace_from_store_dirty();
        self.direct_workspace_dirty = true;
    }

    /// Mark a single scheme as dirty.
    pub fn mark_scheme_dirty(&mut self, scheme_id: SchemeId) {
        self.store.mark_scheme_dirty(scheme_id);
        self.dirty_schemes.insert(scheme_id);
        self.index_dirty = true;
        self.direct_workspace_dirty = true;
    }

    /// Mark only the workspace index as dirty (folder structure changes, etc.)
    pub fn mark_index_dirty(&mut self) {
        self.store.mark_index_dirty();
        self.index_dirty = true;
        self.direct_workspace_dirty = true;
    }

    pub fn mark_direct_workspace_dirty(&mut self) {
        self.direct_workspace_dirty = true;
    }

    /// Returns true if any scheme or the index needs saving.
    pub fn is_dirty(&self) -> bool {
        self.index_dirty || !self.dirty_schemes.is_empty()
    }

    pub fn pending_crdt_edits(&self) -> Vec<PendingCrdtEdit> {
        self.store.pending_crdt_edits()
    }

    pub fn has_pending_crdt_edits(&self) -> bool {
        self.store.has_pending_crdt_edits()
    }

    /// Snapshot the long-lived CRDT documents' persisted state — for durable saving
    /// and for seeding the background sync with this device's latest local edits.
    pub fn crdt_document_states(&self) -> HashMap<DocumentId, Vec<u8>> {
        self.store.crdt_document_states()
    }

    pub fn clear_pushed_crdt_edits(
        &mut self,
        document: DocumentId,
        through_local_sequence: u64,
    ) -> usize {
        self.store
            .clear_pushed_crdt_edits(document, through_local_sequence)
    }

    pub fn sync_store_from_workspace(&mut self) {
        let dirty = WorkspaceDirtyState::from_parts(self.dirty_schemes.clone(), self.index_dirty);
        if self.direct_workspace_dirty {
            self.store
                .replace_workspace(self.workspace.clone(), dirty, false);
            self.direct_workspace_dirty = false;
        } else {
            self.store.replace_dirty_state(dirty);
        }
    }

    pub fn sync_workspace_from_store(&mut self) {
        self.workspace = self.store.workspace().clone();
        self.sync_workspace_from_store_dirty();
        self.direct_workspace_dirty = false;
    }

    /// The search/calendar/channel index over the live workspace, rebuilt lazily
    /// on demand (see [`WorkspaceStore::indexed`]). No render path reads this
    /// today; it exists for query features and is kept off the per-edit hot path.
    pub fn indexed(&mut self) -> &IndexedWorkspace {
        self.store.indexed()
    }

    pub fn sync_workspace_from_store_dirty(&mut self) {
        self.dirty_schemes = self.store.dirty().schemes.clone();
        self.index_dirty = self.store.dirty().index;
    }

    pub fn apply_prechecked_local_command(
        &mut self,
        command: Command,
        origin: CommandOrigin,
    ) -> Result<CommandReceipt, CommandError> {
        self.sync_store_from_workspace();
        let receipt = self.store.apply_prechecked_local(command, origin)?;
        self.sync_workspace_from_store();
        Ok(receipt)
    }

    pub fn replace_workspace(
        &mut self,
        workspace: Workspace,
        today: NaiveDate,
        loaded_start: NaiveDate,
    ) {
        self.store
            .replace_workspace(workspace, WorkspaceDirtyState::default(), true);
        self.sync_workspace_from_store();
        self.undo_store.clear();
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        self.editor_sessions.clear();
        self.retained_completed.clear();

        let daily_queue = DailyQueueState::new(today, loaded_start);
        self.daily_queue = daily_queue.clone();
        self.daily_queue_today = today;
        self.daily_queue_loaded_start = loaded_start;
        self.daily_queue_visible_dates = daily_queue.visible_dates;
        self.daily_queue_loaded_calendar_months = daily_queue.loaded_calendar_months;
    }

    /// Watermark for detecting local edits made while a background sync run is
    /// in flight. Capture it when the run snapshots the workspace and pass it to
    /// [`has_local_edits_since`](Self::has_local_edits_since) when the run lands.
    pub fn local_edit_watermark(&self) -> u64 {
        self.store.local_sequence_watermark()
    }

    pub fn has_local_edits_since(&self, watermark: u64) -> bool {
        self.direct_workspace_dirty || self.store.local_sequence_watermark() != watermark
    }

    /// Merge a sync run's result into the live workspace, preserving edits
    /// applied while the run was in flight (see
    /// [`WorkspaceStore::merge_sync_crdt_states`]). Unlike
    /// [`replace_workspace_from_sync`](Self::replace_workspace_from_sync) the
    /// undo history survives — the local operations it refers to are still in
    /// the merged workspace. Returns false when the merge isn't possible and
    /// the caller must fall back to the replace path.
    pub fn merge_workspace_from_sync(
        &mut self,
        sync_workspace: &Workspace,
        crdt_states: &HashMap<DocumentId, Vec<u8>>,
    ) -> bool {
        // Flush direct (non-command) workspace mutations into the store first so
        // the merge materializes from documents that already carry them.
        self.sync_store_from_workspace();
        if !self
            .store
            .merge_sync_crdt_states(sync_workspace, crdt_states)
        {
            return false;
        }
        self.sync_workspace_from_store();
        true
    }

    pub fn replace_workspace_from_sync(
        &mut self,
        workspace: Workspace,
        crdt_states: HashMap<DocumentId, Vec<u8>>,
    ) {
        // Work out which schemes this sync run actually changed by diffing the
        // incoming content against the current one. The replace path runs only
        // when there are no in-flight local edits, so any item-content difference
        // is precisely the remote delta. (We can't use `crdt_states` for this —
        // it always carries every document, not just the changed ones.) Undo
        // history for schemes the sync didn't touch then survives the replace.
        let mut affected_schemes = std::collections::HashSet::new();
        for (scheme_id, old_scheme) in &self.workspace.schemes {
            match workspace.schemes.get(scheme_id) {
                Some(new_scheme) if new_scheme.items == old_scheme.items => {}
                _ => {
                    affected_schemes.insert(*scheme_id);
                }
            }
        }
        for scheme_id in workspace.schemes.keys() {
            if !self.workspace.schemes.contains_key(scheme_id) {
                affected_schemes.insert(*scheme_id);
            }
        }

        // Apply the run's merged document states incrementally — only the documents
        // that actually changed — instead of reconstructing every CRDT document from
        // scratch. The replace path runs with no in-flight local edits, so applying
        // the merged states on top of the local CRDT yields the same canonical result
        // as a full rebuild, but skips the dominant cost of landing a sync on a large
        // workspace (rebuilding hundreds of unchanged documents). A full rebuild
        // remains the fallback if the incremental merge reports an invalid state.
        self.sync_store_from_workspace();
        if !self.store.merge_sync_crdt_states(&workspace, &crdt_states) {
            let dirty = WorkspaceDirtyState::all(&workspace);
            self.store
                .replace_workspace_with_crdt_states(workspace, dirty, false, crdt_states);
        }
        self.sync_workspace_from_store();

        // Discard undo/redo only for affected schemes and global entries,
        // preserving history for unaffected schemes.
        self.undo_store.clear_affected_by_schemes(&affected_schemes);

        // Clear editor groups only if they're tied to an affected scheme.
        if self
            .editor_undo_group
            .as_ref()
            .is_some_and(|g| affected_schemes.contains(&g.key.scheme_id))
        {
            self.editor_undo_group = None;
        }
        if self
            .recurrence_undo_group
            .as_ref()
            .is_some_and(|g| affected_schemes.contains(&g.key.scheme_id))
        {
            self.recurrence_undo_group = None;
        }
    }
}
