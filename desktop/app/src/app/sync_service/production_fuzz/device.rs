//! A simulated desktop install, driven only through production code: the real
//! `AppState`/store, the app's startup load, save task, shutdown flush, and the
//! real `sync_snapshot` with the sync task's landing — against its own data
//! directory and an in-memory backend.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::NaiveDate;
use knotq_model::{AppSettings, Workspace};
use knotq_state::{daily_queue_default_window_start, AppState};
use knotq_storage_json::{
    load_workspace_with_options, run_pending_upgrades, save_pending_crdt_edits_with_item_fields,
    save_workspace, save_workspace_incremental, WorkspaceLoadOptions,
};

use super::super::landing::{
    adopt_sync_workspace, capture_local_item_edits, clear_pushed_edits, reassert_local_item_edits,
    run_changed_workspace,
};
use super::super::snapshot::sync_snapshot_in;
use super::super::{SyncEnvironment, SyncRunResult, SyncSnapshot};
use super::backend::Account;

pub(super) struct DesktopDevice {
    pub(super) index: usize,
    data_dir: PathBuf,
    pub(super) workspace_path: PathBuf,
    pub(super) image_dir: PathBuf,
    pub(super) state: AppState,
    /// Which fuzz account the device is signed into, if any.
    pub(super) account: Option<usize>,
    /// A sync run has returned and not landed. The save task defers while one is
    /// in flight (see `services::tasks::spawn_save_task`), so no save — and no
    /// partial save before a crash — can happen in that window.
    run_in_flight: bool,
}

/// A sync run that has returned but not yet landed: the UI thread may apply
/// edits before it does.
pub(super) struct InFlightSync {
    pub(super) result: anyhow::Result<SyncRunResult>,
    watermark: u64,
}

impl InFlightSync {
    /// A one-line account of what the run did, for traces.
    pub(super) fn summary(&self) -> String {
        match &self.result {
            Ok(result) => format!(
                "pushed {} doc(s), applied {} remote update(s), {} pending left, local change {}",
                result.pushed.len(),
                result.remote_updates_applied,
                result.remaining_pending,
                result.local_workspace_changed,
            ),
            Err(err) => format!("failed: {err:#}"),
        }
    }
}

/// How far a save got before the process died.
#[derive(Clone, Copy, Debug)]
pub(super) enum CrashPoint {
    /// Before anything was written.
    BeforeSave,
    /// The workspace and scheme files were written; the pending queue and the
    /// CRDT states were not.
    AfterWorkspace,
    /// Everything but the CRDT document states.
    AfterPending,
}

impl DesktopDevice {
    /// A first launch on an empty data directory: the app seeds its starter
    /// workspace, exactly as a new install does.
    pub(super) fn install(index: usize, data_dir: PathBuf, today: NaiveDate) -> Self {
        let settings = AppSettings::default();
        Self::launch(index, data_dir, settings, today, None)
    }

    /// A first launch on a data directory that already holds `workspace` (and
    /// nothing else — no CRDT state, no sync state), so the app loads it instead
    /// of seeding the starter workspace.
    pub(super) fn install_with(
        index: usize,
        data_dir: PathBuf,
        today: NaiveDate,
        workspace: &Workspace,
    ) -> Self {
        let workspace_path = data_dir.join("workspace").join("workspace.json");
        save_workspace(&workspace_path, workspace).expect("write the pre-launch workspace");
        Self::launch(index, data_dir, AppSettings::default(), today, None)
    }

    fn launch(
        index: usize,
        data_dir: PathBuf,
        settings: AppSettings,
        today: NaiveDate,
        account: Option<usize>,
    ) -> Self {
        let workspace_path = data_dir.join("workspace").join("workspace.json");
        let image_dir = data_dir.join("workspace").join("assets").join("images");
        // `bootstrap::upgraded_data_directory`, then `KnotQApp::new`.
        let _ = run_pending_upgrades(&workspace_path);
        let bootstrap = crate::app::bootstrap::load_or_seed_from_path(&workspace_path, today);
        assert!(
            bootstrap.save_blocked_reason.is_none(),
            "device {index}: workspace load failed: {:?}",
            bootstrap.save_blocked_reason
        );
        let crdt_states = crate::app::constructor::restored_crdt_states(&workspace_path);
        let initial_sequence = crate::app::constructor::restored_initial_sequence(&workspace_path);
        let state = AppState::new(
            bootstrap.workspace,
            settings,
            today,
            daily_queue_default_window_start(today),
            true,
            crdt_states,
            initial_sequence,
        );
        Self {
            index,
            data_dir,
            workspace_path,
            image_dir,
            state,
            account,
            run_in_flight: false,
        }
    }

    pub(super) fn today(&self) -> NaiveDate {
        self.state.daily_queue_today
    }

    pub(super) fn set_today(&mut self, today: NaiveDate) {
        self.state.daily_queue_today = today;
        self.state.daily_queue_loaded_start = daily_queue_default_window_start(today);
    }

    /// Everything the user could reach: the in-memory workspace plus the days
    /// that live only on disk because they have not been paged in.
    pub(super) fn full_workspace(&self) -> Workspace {
        let mut workspace = self.state.workspace.clone();
        let missing: Vec<_> = workspace
            .daily_queue
            .values()
            .filter(|id| !workspace.schemes.contains_key(id))
            .copied()
            .collect();
        if missing.is_empty() {
            return workspace;
        }
        if let Ok(Some(disk)) =
            load_workspace_with_options(&self.workspace_path, WorkspaceLoadOptions::all())
        {
            for id in missing {
                if let Some(scheme) = disk.schemes.get(&id) {
                    workspace.schemes.insert(id, scheme.clone());
                }
            }
        }
        workspace
    }

    // --- persistence --------------------------------------------------------------

    /// One run of the save task (`services::tasks::spawn_save_task`).
    pub(super) fn save(&mut self) -> anyhow::Result<()> {
        if !self.state.is_dirty() || self.run_in_flight {
            return Ok(());
        }
        let pending = self.state.pending_crdt_edits();
        let queued_item_fields = self.state.queued_item_fields();
        let (scope, handles) = self.state.take_crdt_save_scope();
        let dirty_ids = std::mem::take(&mut self.state.dirty_schemes);
        self.state.index_dirty = false;
        let workspace = self.state.workspace.clone();
        let crdt_states: HashMap<_, _> = handles
            .into_iter()
            .map(|(document, handle)| (document, handle.encode()))
            .collect();
        let result = crate::app::services::write_save_snapshot(
            &self.workspace_path,
            &workspace,
            &dirty_ids,
            &pending,
            &queued_item_fields,
            scope,
            &crdt_states,
        );
        if result.is_err() {
            self.state.dirty_schemes.extend(dirty_ids);
            self.state.index_dirty = true;
            self.state.mark_all_crdt_documents_changed();
        }
        result
    }

    /// Quit cleanly (`flush_for_shutdown`) and launch again.
    pub(super) fn relaunch(self) -> Self {
        let Self {
            index,
            data_dir,
            workspace_path,
            mut state,
            account,
            run_in_flight,
            ..
        } = self;
        // `flush_for_shutdown` first completes elapsed events; the world runs
        // that as a recorded local step (`World::relaunch`) before calling here.
        // Like the app, a quit before the run lands forgets the run's pulls.
        if run_in_flight {
            crate::app::services::abandon_unlanded_sync_run(&workspace_path);
        }
        crate::app::services::write_shutdown_workspace(&workspace_path, &mut state)
            .expect("shutdown flush");
        let settings = state.settings.clone();
        let today = state.daily_queue_today;
        drop(state);
        Self::launch(index, data_dir, settings, today, account)
    }

    /// Die mid-save and launch again. Whatever was not written is gone — the
    /// in-memory state is simply dropped.
    pub(super) fn crash(self, point: CrashPoint) -> Self {
        let Self {
            index,
            data_dir,
            workspace_path,
            mut state,
            account,
            run_in_flight,
            ..
        } = self;
        // The save task never runs while a sync is in flight, so a crash then
        // cannot have written part of a save.
        let point = if run_in_flight {
            CrashPoint::BeforeSave
        } else {
            point
        };
        match point {
            CrashPoint::BeforeSave => {}
            CrashPoint::AfterWorkspace | CrashPoint::AfterPending => {
                let dirty_ids = std::mem::take(&mut state.dirty_schemes);
                let workspace = state.workspace.clone();
                let _ = if dirty_ids.is_empty() {
                    save_workspace(&workspace_path, &workspace)
                } else {
                    save_workspace_incremental(&workspace_path, &workspace, &dirty_ids)
                };
                if matches!(point, CrashPoint::AfterPending) {
                    let _ = save_pending_crdt_edits_with_item_fields(
                        &workspace_path,
                        &state.pending_crdt_edits(),
                        &state.queued_item_fields(),
                    );
                }
            }
        }
        let settings = state.settings.clone();
        let today = state.daily_queue_today;
        drop(state);
        Self::launch(index, data_dir, settings, today, account)
    }

    // --- accounts ----------------------------------------------------------------

    pub(super) fn sign_in(&mut self, account: &Account) {
        self.state.settings.sync_account = Some(account.settings());
        self.account = Some(account.index);
    }

    /// `sign_out_sync_account`: forget the session, keep the data.
    pub(super) fn sign_out(&mut self) {
        self.state.settings.sync_account = None;
        self.account = None;
    }

    // --- sync ----------------------------------------------------------------------

    /// The UI-thread snapshot and the background run of `run_sync_attempt`.
    /// Returns `None` when the device is not signed in.
    pub(super) fn run_sync(
        &mut self,
        account: &Account,
        allow_squash: bool,
    ) -> Option<InFlightSync> {
        let account_settings = self.state.settings.sync_account.clone()?;
        self.state.sync_store_from_workspace();
        let pending = self.state.pending_crdt_edits();
        let crdt_states = self.state.crdt_document_state_handles();
        let snapshot = SyncSnapshot {
            workspace: self.state.workspace.clone(),
            account: account_settings,
            replica_id: self.state.settings.replica_id,
            pending,
            queued_item_fields: self.state.queued_item_fields(),
            crdt_states,
            notification_defaults: self.state.settings.notification_defaults,
            reuse_schedule: None,
            ws_sync: None,
            allow_squash,
        };
        let watermark = self.state.local_edit_watermark();
        let result = sync_snapshot_in(
            SyncEnvironment {
                workspace_path: &self.workspace_path,
                image_dir: &self.image_dir,
                transport: &account.server,
                side_channel: &account.server,
            },
            snapshot,
        );
        self.run_in_flight = true;
        Some(InFlightSync { result, watermark })
    }

    /// Land a finished run the way the sync task's UI-thread closure does, then
    /// run the save it signals. Returns the run's error, if it failed.
    pub(super) fn land_sync(&mut self, run: InFlightSync) -> Option<anyhow::Error> {
        self.run_in_flight = false;
        match run.result {
            Ok(result) => {
                let local_item_edits =
                    capture_local_item_edits(&self.state, &result.queued_item_fields);
                clear_pushed_edits(&mut self.state, &result.pushed, run.watermark);
                if run_changed_workspace(
                    result.remote_updates_applied,
                    result.local_workspace_changed,
                ) {
                    adopt_sync_workspace(
                        &mut self.state,
                        result.workspace,
                        result.crdt_states,
                        run.watermark,
                        result.squash_applied,
                    );
                    reassert_local_item_edits(&mut self.state, local_item_edits);
                }
                let _ = self.save();
                None
            }
            Err(err) => Some(err),
        }
    }

    /// A full sync attempt with the scheduler's one epoch-stale retry.
    pub(super) fn sync_now(
        &mut self,
        account: &Account,
        allow_squash: bool,
    ) -> Option<anyhow::Error> {
        for attempt in 0..2 {
            let run = self.run_sync(account, allow_squash)?;
            let err = self.land_sync(run)?;
            let epoch_stale = err
                .downcast_ref::<knotq_sync::SyncPushEpochStale>()
                .is_some();
            if attempt == 0 && epoch_stale {
                continue;
            }
            return Some(err);
        }
        None
    }

    /// Unpushed work in memory or on disk.
    pub(super) fn pending_edit_count(&mut self) -> usize {
        let on_disk = knotq_storage_json::load_local_sync_state(&self.workspace_path)
            .map(|state| state.pending.len())
            .unwrap_or(0);
        self.state.pending_crdt_edits().len().max(on_disk)
    }
}
