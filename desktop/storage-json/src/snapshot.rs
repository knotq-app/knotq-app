//! Restorable snapshots of the whole data directory.
//!
//! Distinct from `upgrade/backup.rs`, which protects one migration's declared
//! path set and is thrown away once the migration sticks. This is the copy a
//! *user* falls back to: "put my workspace back the way it was yesterday".
//!
//! # Why this copies both halves of the data directory
//!
//! The data directory has two halves — the plain files the app reads
//! (`workspace.json`, `schemes/`, `daily_queue/`) and the CRDT document states
//! sync merges into (`sync-crdt-state/`, `sync-state.json`). They are not
//! independent: on every launch `reconcile_workspace_from_documents` rebuilds
//! the plain workspace *from the documents*, because the documents are what
//! sync converges on.
//!
//! So a snapshot of the plain files alone is not restorable. Put it back and
//! the next launch reconciles it away, silently, and the user watches their
//! recovery undo itself. The existing `backups/<weekday>/` copy has exactly
//! that shape and is a diagnostic aid, not a recovery path.
//!
//! Kept as dumb as the migration backups for the same reason: it has to be
//! readable by a human on a support call and restorable by hand with a file
//! manager, so nothing is packed, compressed or renamed.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const SNAPSHOT_ROOT: &str = "snapshots";
/// One per day, a week deep. The rotation key is the date, so a day with a
/// hundred saves still costs one copy.
const KEEP: usize = 7;

/// Directories that must never be copied *into* a snapshot: the snapshot roots
/// themselves (which would recurse), and logs, which are large, are not user
/// content, and are actively being written while the copy runs.
const EXCLUDED: &[&str] = &["snapshots", "backups", "upgrade-backups", "logs"];

/// A snapshot on disk, newest first when listed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoverySnapshot {
    /// The directory holding the copy.
    pub dir: PathBuf,
    /// `YYYY-MM-DD`, which is also the rotation key.
    pub day: String,
}

/// Take today's snapshot if it has not been taken yet, then prune old ones.
///
/// A no-op for the rest of the day once taken, so this is safe to call from a
/// save path: the cost is one `exists()` on every call but the first.
pub fn capture_daily_snapshot(data_dir: &Path) -> Result<Option<RecoverySnapshot>> {
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let dir = data_dir.join(SNAPSHOT_ROOT).join(&day);
    if dir.exists() {
        return Ok(None);
    }
    // Copy into a temporary directory and rename it into place, so a snapshot
    // interrupted halfway (a crash, a power cut) never becomes a listed one.
    // A half-copied snapshot offered as a recovery point is worse than none.
    let staging = data_dir.join(SNAPSHOT_ROOT).join(format!(".{day}.partial"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).with_context(|| format!("create {}", staging.display()))?;
    copy_tree(data_dir, &staging)?;
    fs::rename(&staging, &dir).with_context(|| format!("publish snapshot {}", dir.display()))?;
    prune(data_dir)?;
    Ok(Some(RecoverySnapshot { dir, day }))
}

/// Every snapshot on disk, newest first.
pub fn list_snapshots(data_dir: &Path) -> Vec<RecoverySnapshot> {
    let root = data_dir.join(SNAPSHOT_ROOT);
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut snapshots: Vec<RecoverySnapshot> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let day = entry.file_name().to_string_lossy().into_owned();
            // Skip staging directories and anything not named like a date.
            (day.len() == 10 && day.chars().all(|c| c.is_ascii_digit() || c == '-')).then(|| {
                RecoverySnapshot {
                    dir: entry.path(),
                    day,
                }
            })
        })
        .collect();
    snapshots.sort_by(|left, right| right.day.cmp(&left.day));
    snapshots
}

/// Put `snapshot` back, moving what is there now aside first.
///
/// Never deletes: the displaced copy goes to `snapshots/replaced-<timestamp>/`,
/// so a restore chosen by mistake is itself recoverable. The caller is
/// responsible for making sure nothing is writing to the data directory — in
/// practice this runs at launch, before anything is loaded.
pub fn restore_snapshot(data_dir: &Path, snapshot: &RecoverySnapshot) -> Result<PathBuf> {
    if !snapshot.dir.exists() {
        anyhow::bail!("snapshot {} is gone", snapshot.dir.display());
    }
    let displaced = data_dir.join(SNAPSHOT_ROOT).join(format!(
        "replaced-{}",
        chrono::Local::now().format("%Y%m%dT%H%M%S")
    ));
    fs::create_dir_all(&displaced).with_context(|| format!("create {}", displaced.display()))?;
    copy_tree(data_dir, &displaced)?;

    // Remove the live copy of everything the snapshot replaces, so a file the
    // snapshot does not have does not survive the restore and leave the
    // directory describing two different workspaces.
    for entry in fs::read_dir(data_dir).with_context(|| format!("read {}", data_dir.display()))? {
        let entry = entry?;
        if is_excluded(&entry.file_name().to_string_lossy()) {
            continue;
        }
        remove_any(&entry.path())?;
    }
    copy_tree(&snapshot.dir, data_dir)?;
    Ok(displaced)
}

/// The file recording that the user asked for a restore.
const RESTORE_REQUEST: &str = ".restore-request";

/// Ask for `snapshot` to be restored the next time the app starts.
///
/// Restoring cannot happen while the app is running: the store holds the
/// workspace and the CRDT documents in memory and would write them straight
/// back over the restored files, and the sync task may be mid-run against the
/// half it no longer matches. So the UI records the request and the restore
/// happens at launch, before anything has read the directory.
pub fn request_restore(data_dir: &Path, snapshot: &RecoverySnapshot) -> Result<()> {
    let root = data_dir.join(SNAPSHOT_ROOT);
    fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
    fs::write(root.join(RESTORE_REQUEST), &snapshot.day)
        .with_context(|| format!("record a restore of {}", snapshot.day))
}

/// Forget a restore the user asked for and then thought better of.
pub fn cancel_restore(data_dir: &Path) {
    let _ = fs::remove_file(data_dir.join(SNAPSHOT_ROOT).join(RESTORE_REQUEST));
}

/// The day a restore is pending for, if one is.
pub fn pending_restore(data_dir: &Path) -> Option<String> {
    let day = fs::read_to_string(data_dir.join(SNAPSHOT_ROOT).join(RESTORE_REQUEST)).ok()?;
    let day = day.trim().to_string();
    (!day.is_empty()).then_some(day)
}

/// Carry out a pending restore, if there is one. Call at launch, before
/// anything reads the data directory.
///
/// The request is cleared *before* the copy rather than after. A restore that
/// fails halfway and then runs again on the next launch would take a second
/// snapshot of the half-restored directory as the "replaced" copy, and the
/// user's real state would be one more step away each time they tried. One
/// attempt, reported either way.
pub fn take_pending_restore(data_dir: &Path) -> Option<Result<PathBuf>> {
    let day = pending_restore(data_dir)?;
    cancel_restore(data_dir);
    let snapshot = list_snapshots(data_dir)
        .into_iter()
        .find(|snapshot| snapshot.day == day)?;
    Some(restore_snapshot(data_dir, &snapshot))
}

fn prune(data_dir: &Path) -> Result<()> {
    let snapshots = list_snapshots(data_dir);
    for stale in snapshots.into_iter().skip(KEEP) {
        let _ = fs::remove_dir_all(&stale.dir);
    }
    Ok(())
}

fn is_excluded(name: &str) -> bool {
    EXCLUDED.contains(&name) || name.starts_with('.')
}

/// Copy every child of `from` into `to`, skipping the excluded roots.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if is_excluded(&name.to_string_lossy()) {
            continue;
        }
        copy_recursive(&entry.path(), &to.join(name))?;
    }
    Ok(())
}

fn copy_recursive(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
        for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::copy(from, to).with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn remove_any(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}
