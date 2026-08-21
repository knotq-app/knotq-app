//! Running an existing install's data directory forward onto this build.
//!
//! # Why this exists
//!
//! Everything in the data directory belongs to the user and most of it cannot be
//! recreated: the schemes are their writing, and `sync-crdt-state/` carries the
//! Yjs identity that lets their other devices recognise this one. A format
//! change that goes wrong does not fail loudly on the developer's machine — it
//! fails months later on a stranger's, as an empty workspace or an account that
//! silently re-seeds itself from nothing.
//!
//! Before this module, format changes were performed implicitly by whichever
//! code path happened to run first (the per-document CRDT state directory was
//! written by the first *save*, which is why an interrupted one used to lose
//! every document). This module makes an upgrade an explicit, ordered,
//! recoverable step that runs once at startup, before anything reads or writes
//! user data.
//!
//! # The guarantees
//!
//! Every migration registered here is run under the same contract, so a new one
//! inherits all of it:
//!
//! 1. **Backed up first.** A migration declares the paths it touches; they are
//!    copied aside before it runs, and restored if it fails. A migration whose
//!    backup cannot be taken does not run.
//! 2. **Verified before it counts.** `apply` is followed by `verify`, which
//!    re-reads from disk. A migration that cannot prove its result is treated as
//!    a failure and rolled back.
//! 3. **Idempotent and crash-safe.** A journal entry is written before `apply`
//!    and cleared after `verify`. Finding one at startup means a previous run
//!    died part-way through, so the migration runs again — which is why `apply`
//!    must be safe to run twice, and why the tests assert exactly that.
//! 4. **Never destructive.** The pre-migration form is moved aside, not deleted.
//! 5. **Ordered and recorded.** `data-layout.json` names the migrations that have
//!    completed, so a migration is attempted once rather than sniffing the disk
//!    forever.
//!
//! # Reading a directory written by a newer build
//!
//! Users downgrade — a rollback, a second machine on an older version, a synced
//! folder. `DATA_LAYOUT_VERSION` is recorded on every run, and a directory whose
//! recorded version is higher than this build's is reported as
//! [`UpgradeReport::written_by_newer_build`]. The caller must then run without
//! saving: the alternative is this build rewriting files in a format it only
//! half understands, which is how a downgrade turns into permanent loss.
//!
//! # Adding a migration
//!
//! Append a [`Migration`] to `migrations::ALL` (never renumber or reorder — the
//! recorded ids are on users' disks), bump [`DATA_LAYOUT_VERSION`], and add a
//! captured fixture of the format you are migrating *from* under
//! `tests/fixtures/`. `tests/upgrade_framework.rs` will then exercise your
//! migration for idempotency, crash-recovery and rollback without you writing
//! those tests yourself.

mod backup;
mod migrations;
mod record;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use record::{data_layout_path, DataLayoutRecord, DATA_LAYOUT_VERSION};

use crate::sync_state::sync_state_data_dir;

/// The locations a migration is allowed to reason about.
///
/// Migrations take this rather than a bare path so they cannot disagree about
/// where the data directory is — the workspace file lives one level down inside
/// it, and getting that wrong once meant writing sync state into the wrong
/// place.
pub struct DataPaths {
    /// The platform data directory: `settings.json`, `sync-state.json`, and the
    /// CRDT state live directly in here.
    pub data_dir: PathBuf,
    /// `<data_dir>/workspace/workspace.json`.
    pub workspace_path: PathBuf,
}

impl DataPaths {
    pub fn for_workspace(workspace_path: &Path) -> Self {
        Self {
            data_dir: sync_state_data_dir(workspace_path),
            workspace_path: workspace_path.to_path_buf(),
        }
    }
}

/// One format change, run at most once per data directory.
///
/// `apply` must be idempotent: a crash between `apply` and the journal being
/// cleared makes it run again on the next launch.
pub struct Migration {
    /// Stable identifier, recorded on disk. Never change one that has shipped.
    pub id: &'static str,
    /// What it does, for the log line a support conversation will quote.
    pub summary: &'static str,
    /// Everything `apply` may write to or remove, backed up before it runs.
    /// Paths that do not exist are skipped; directories are copied recursively.
    /// Keep this bounded — it is copied in full.
    pub paths: fn(&DataPaths) -> Vec<PathBuf>,
    /// Whether the old form is present. Checked even for a migration already
    /// recorded as applied, so a directory that was rolled back to the old form
    /// (a restore from backup, a downgrade) is migrated again rather than left.
    pub is_pending: fn(&DataPaths) -> Result<bool>,
    pub apply: fn(&DataPaths) -> Result<()>,
    /// Re-read from disk and prove the new form carries what the old one did.
    /// Runs before the migration is recorded; failing it rolls the change back.
    pub verify: fn(&DataPaths) -> Result<()>,
}

#[derive(Debug, Default)]
pub struct UpgradeReport {
    /// Migrations that ran, in order.
    pub applied: Vec<&'static str>,
    /// Migrations that failed and were rolled back, with the reason. The data
    /// directory is back in its pre-migration form for each of these.
    pub failed: Vec<(&'static str, String)>,
    /// Migrations that were re-run because a previous attempt did not finish.
    pub resumed: Vec<&'static str>,
    /// Set when the data directory records a layout version this build does not
    /// know. The caller must not save over it.
    pub written_by_newer_build: Option<u32>,
}

impl UpgradeReport {
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty() && self.written_by_newer_build.is_none()
    }

    /// A one-line summary for the log, or `None` when nothing happened.
    pub fn log_line(&self) -> Option<String> {
        if self.applied.is_empty() && self.failed.is_empty() && self.resumed.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.applied.is_empty() {
            parts.push(format!("applied [{}]", self.applied.join(", ")));
        }
        if !self.resumed.is_empty() {
            parts.push(format!("resumed [{}]", self.resumed.join(", ")));
        }
        for (id, err) in &self.failed {
            parts.push(format!("FAILED and rolled back: {id} ({err})"));
        }
        Some(format!("data upgrade: {}", parts.join("; ")))
    }
}

/// Bring the data directory up to this build's format. Call once at startup,
/// before anything loads or saves user data.
///
/// Never returns `Err`: a data directory that cannot be upgraded must still open
/// (the loaders are written to cope with the old form), so problems are reported
/// rather than raised. `UpgradeReport::is_clean` being false means the caller
/// should keep the session read-only.
pub fn run_pending_upgrades(workspace_path: &Path) -> UpgradeReport {
    let paths = DataPaths::for_workspace(workspace_path);
    let mut report = UpgradeReport::default();

    // Nothing on disk yet: a fresh install is already in this build's format.
    if !paths.data_dir.exists() {
        return report;
    }

    let mut layout = match record::load(&paths.data_dir) {
        Ok(record) => record,
        Err(err) => {
            // An unreadable record must not make us re-run migrations blindly
            // against data that may already be migrated; `is_pending` still
            // gates every one of them, so starting from a blank record is safe.
            eprintln!("data layout record unreadable ({err:#}); treating as unrecorded");
            DataLayoutRecord::default()
        }
    };

    if layout.layout_version > DATA_LAYOUT_VERSION {
        report.written_by_newer_build = Some(layout.layout_version);
        return report;
    }

    let unfinished = record::take_unfinished(&paths.data_dir);
    for migration in migrations::ALL {
        let resumed = unfinished.iter().any(|id| id == migration.id);
        let pending = match (migration.is_pending)(&paths) {
            Ok(pending) => pending,
            Err(err) => {
                report
                    .failed
                    .push((migration.id, format!("could not be checked: {err:#}")));
                continue;
            }
        };
        if !pending {
            layout.mark_applied(migration.id);
            continue;
        }
        if resumed {
            report.resumed.push(migration.id);
        }
        match run_one(migration, &paths) {
            Ok(()) => {
                layout.mark_applied(migration.id);
                report.applied.push(migration.id);
            }
            Err(err) => report.failed.push((migration.id, format!("{err:#}"))),
        }
    }

    layout.layout_version = DATA_LAYOUT_VERSION;
    layout.last_written_by = env!("CARGO_PKG_VERSION").to_string();
    if let Err(err) = record::save(&paths.data_dir, &layout) {
        eprintln!("could not record the data layout version: {err:#}");
    }
    backup::prune(&paths.data_dir);
    report
}

fn run_one(migration: &Migration, paths: &DataPaths) -> Result<()> {
    let targets = (migration.paths)(paths);
    let snapshot = backup::create(&paths.data_dir, migration.id, &targets)
        .with_context(|| format!("back up before {}", migration.id))?;

    // Written before the change and cleared after it is verified, so a process
    // killed mid-migration is recognised on the next launch.
    record::begin(&paths.data_dir, migration.id)?;

    let outcome = (migration.apply)(paths)
        .with_context(|| format!("apply {}", migration.id))
        .and_then(|()| {
            (migration.verify)(paths).with_context(|| format!("verify {}", migration.id))
        });

    match outcome {
        Ok(()) => {
            record::finish(&paths.data_dir, migration.id);
            Ok(())
        }
        Err(err) => {
            // Put the directory back the way it was before reporting: leaving a
            // half-migrated directory behind is worse than not migrating.
            if let Err(restore_err) = backup::restore(&snapshot) {
                record::finish(&paths.data_dir, migration.id);
                return Err(err.context(format!(
                    "and the rollback also failed ({restore_err:#}); \
                     a copy of the original files is at {}",
                    snapshot.dir.display()
                )));
            }
            record::finish(&paths.data_dir, migration.id);
            Err(err)
        }
    }
}

/// The registered migrations, oldest first. Exposed so the upgrade tests can
/// hold the registry to its own rules — ids unique and stable, and
/// [`DATA_LAYOUT_VERSION`] bumped whenever one is added.
pub fn registered_migrations() -> &'static [Migration] {
    migrations::ALL
}
