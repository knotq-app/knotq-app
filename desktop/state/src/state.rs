use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use knotq_commands::{ChangeSet, Command, CommandError, CommandOrigin, CommandReceipt};
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
    // Monotonic counter bumped by every route that can change `workspace` or
    // `retained_completed`.
    content_revision: u64,
    // As `content_revision`, but a command that only rewrites item body text
    // does not bump it. The upcoming panel's row set (which items are due, in
    // what order) cannot change under such an edit, so it can keep its scan and
    // re-read just the text of the handful of rows it is showing — which is what
    // makes typing cheap, since typing is exactly that kind of command.
    schedule_revision: u64,
    // Per-scheme view of `schedule_revision`: the value it held when a change
    // *known to be confined to that scheme* last landed. A change whose scope is
    // not known instead raises `all_schemes_schedule_revision`, so forgetting to
    // narrow is conservative (everything looks changed) rather than stale.
    scheme_schedule_revisions: HashMap<SchemeId, u64>,
    all_schemes_schedule_revision: u64,

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
    pub fn new<B: AsRef<[u8]>>(
        workspace: Workspace,
        settings: AppSettings,
        today: NaiveDate,
        loaded_start: NaiveDate,
        initial_dirty: bool,
        crdt_states: HashMap<DocumentId, B>,
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
            content_revision: 0,
            schedule_revision: 0,
            scheme_schedule_revisions: HashMap::new(),
            all_schemes_schedule_revision: 0,
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

    /// The canonical workspace held by the store, which `self.workspace` is a
    /// working copy of. Exposed so tests can assert the two stay in step.
    pub fn store_workspace(&self) -> &Workspace {
        self.store.workspace()
    }

    /// Revision of everything the workspace-derived views read. Bumped by every
    /// mutation route, so a view can cache a projection of the workspace and
    /// reuse it while this is unchanged.
    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    /// Revision of the workspace's *schedule* — which items are dated, repeating,
    /// done, and in what order. Unlike [`content_revision`](Self::content_revision)
    /// this survives a text-only edit.
    pub fn schedule_revision(&self) -> u64 {
        self.schedule_revision
    }

    /// [`schedule_revision`](Self::schedule_revision) as seen by a single scheme:
    /// unchanged while every schedule change lands somewhere else.
    ///
    /// A view that derives something per scheme — the upcoming panel expands each
    /// item's recurrence, which is by far its most expensive work — can hold that
    /// per-scheme result and rebuild only the schemes whose value moved. Pressing
    /// Enter changes one scheme's schedule; without this it would invalidate all
    /// of them.
    pub fn scheme_schedule_revision(&self, scheme: SchemeId) -> u64 {
        self.all_schemes_schedule_revision.max(
            self.scheme_schedule_revisions
                .get(&scheme)
                .copied()
                .unwrap_or(0),
        )
    }

    fn bump_content_revision(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
        self.schedule_revision = self.schedule_revision.wrapping_add(1);
        // Scope unknown: assume every scheme moved.
        self.all_schemes_schedule_revision = self.schedule_revision;
    }

    /// Bump only the content revision, for a change that provably left the
    /// schedule alone.
    fn bump_content_revision_text_only(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
    }

    /// Bump for a change whose effect is known to be confined to `schemes`.
    ///
    /// Only sound where the caller holds a [`ChangeSet`] that fully describes the
    /// change — the same guarantee
    /// [`sync_workspace_from_store_reusing_untouched`](Self::sync_workspace_from_store_reusing_untouched)
    /// already debug-asserts before reusing the untouched `Scheme` values.
    fn bump_content_revision_scoped(&mut self, schemes: &[SchemeId]) {
        self.content_revision = self.content_revision.wrapping_add(1);
        self.schedule_revision = self.schedule_revision.wrapping_add(1);
        for scheme in schemes {
            self.scheme_schedule_revisions
                .insert(*scheme, self.schedule_revision);
        }
    }

    pub fn retained_completed_mut(&mut self) -> &mut RetainedCompletedItems {
        // Handing out `&mut` is itself the mutation for revision purposes: the
        // caller took it in order to insert, remove, or purge.
        self.bump_content_revision();
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
        self.bump_content_revision();
    }

    /// Mark a single scheme as dirty.
    pub fn mark_scheme_dirty(&mut self, scheme_id: SchemeId) {
        self.store.mark_scheme_dirty(scheme_id);
        self.dirty_schemes.insert(scheme_id);
        self.index_dirty = true;
        self.direct_workspace_dirty = true;
        self.bump_content_revision();
    }

    /// Mark only the workspace index as dirty (folder structure changes, etc.)
    pub fn mark_index_dirty(&mut self) {
        self.store.mark_index_dirty();
        self.index_dirty = true;
        self.direct_workspace_dirty = true;
        self.bump_content_revision();
    }

    pub fn mark_direct_workspace_dirty(&mut self) {
        self.direct_workspace_dirty = true;
        self.bump_content_revision();
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
    pub fn crdt_document_states(&self) -> HashMap<DocumentId, std::sync::Arc<[u8]>> {
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
        self.bump_content_revision();
    }

    /// Refresh the local workspace copy after a command, carrying over the
    /// `Scheme` values the command did not touch instead of deep-cloning them.
    ///
    /// The two copies were identical before the command (`sync_store_from_workspace`
    /// flushes any direct mutation into the store first), and the command's
    /// effect is confined to `touched` — the same guarantee the incremental save
    /// already relies on to decide which scheme files to write. Cloning only
    /// those schemes turns a per-keystroke copy of the entire workspace into a
    /// copy of the one scheme being edited.
    fn sync_workspace_from_store_reusing_untouched(
        &mut self,
        touched: &ChangeSet,
        text_only: bool,
    ) {
        let mut carried_over = std::mem::take(&mut self.workspace.schemes);
        let store = self.store.workspace();
        let mut next = store.clone_without_schemes();
        next.schemes.reserve(store.schemes.len());
        for (id, scheme) in &store.schemes {
            let reused = if touched.schemes.contains(id) {
                None
            } else {
                carried_over.remove(id)
            };
            next.schemes
                .insert(*id, reused.unwrap_or_else(|| scheme.clone()));
        }
        // A `touched` set that under-reported would leave a stale scheme here,
        // and the next direct-mutation flush would push that stale copy back
        // into the store as if it were an edit. Catch any such drift in debug
        // builds and every test run rather than let it corrupt data silently.
        debug_assert_eq!(
            &next,
            store,
            "reusing untouched schemes diverged from the store; `touched` under-reported"
        );
        self.workspace = next;
        self.sync_workspace_from_store_dirty();
        self.direct_workspace_dirty = false;
        if text_only {
            self.bump_content_revision_text_only();
        } else if touched.folders.is_empty() {
            // Schemes only: `touched` is exactly the set that differs, as the
            // assertion above just checked. A folder in the change set means
            // workspace-level structure moved too (which scheme is the daily
            // queue, what is deleted), and that is not confined to any scheme.
            self.bump_content_revision_scoped(&touched.schemes);
        } else {
            self.bump_content_revision();
        }
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
        let text_only = command.changes_only_item_text();
        let receipt = self.store.apply_prechecked_local(command, origin)?;
        self.sync_workspace_from_store_reusing_untouched(&receipt.touched, text_only);
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
    pub fn merge_workspace_from_sync<B: AsRef<[u8]>>(
        &mut self,
        sync_workspace: &Workspace,
        crdt_states: &HashMap<DocumentId, B>,
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

    /// Adopt a sync run's merged workspace, reporting whether it differs at all
    /// from what the UI is already showing.
    ///
    /// A `false` return means this run was a no-op for the user: the caller can
    /// then skip the scroll restore, the dependent service work, and the repaint
    /// it would otherwise force. That is the common case rather than a rare one —
    /// a push leaves the pull cursor one behind the server, so the *next* pull
    /// hands the pusher its own just-pushed document straight back. While typing
    /// over a live socket that self-echo lands continuously, and repainting the
    /// whole window for it is what pins the main thread in `nextDrawable`
    /// instead of servicing key events.
    pub fn replace_workspace_from_sync<B: AsRef<[u8]>>(
        &mut self,
        workspace: Workspace,
        crdt_states: HashMap<DocumentId, B>,
    ) -> bool {
        // Everything the UI renders, compared before anything is mutated.
        // `schemes` covers item content plus per-scheme metadata (name, colour,
        // source); `clone_without_schemes` covers the rest of the workspace and
        // is exhaustively destructured, so a field added later is compared here
        // instead of being silently dropped from the check.
        let visible_change = self.workspace.schemes != workspace.schemes
            || self.workspace.clone_without_schemes() != workspace.clone_without_schemes();

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

        visible_change
    }
}
