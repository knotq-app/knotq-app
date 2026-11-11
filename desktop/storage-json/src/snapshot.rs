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

/// Top-level entries that must never be copied *into* a snapshot: the snapshot
/// and backup roots themselves (which would recurse or duplicate), logs (not
/// user content, written while the copy runs) and staged updates.
const EXCLUDED: &[&str] = &["snapshots", "backups", "upgrade-backups", "logs", "updates"];
/// The same, deeper in the tree, as paths relative to the data directory. The
/// workspace's own history store and weekday backups are each bigger than the
/// workspace, and are their own recovery paths.
const EXCLUDED_NESTED: &[&str] = &["workspace/.knotq-history", "workspace/backups"];
/// Name fragments of files the app sets aside rather than deletes (an old form
/// after a migration, a quarantined unreadable file) and rotated logs. Kept on
/// disk, never multiplied by seven.
const EXCLUDED_FRAGMENTS: &[&str] = &[".migrated", ".unreadable-", ".corrupt", ".log", ".tmp"];

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
    clear_tree(data_dir, Path::new(""))?;
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

/// Whether `rel` (relative to the data directory) stays out of snapshots and
/// is left alone by a restore.
fn is_excluded(rel: &Path) -> bool {
    let Some(name) = rel.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    if EXCLUDED_NESTED.iter().any(|path| rel == Path::new(path)) {
        return true;
    }
    if rel.parent() != Some(Path::new("")) {
        return false;
    }
    EXCLUDED.contains(&name.as_ref())
        || name.starts_with('.')
        || EXCLUDED_FRAGMENTS
            .iter()
            .any(|fragment| name.contains(fragment))
}

/// Whether some excluded path lives below directory `rel`, so clearing it must
/// recurse instead of removing it whole.
fn holds_excluded(rel: &Path) -> bool {
    EXCLUDED_NESTED.iter().any(|path| {
        let path = Path::new(path);
        path != rel && path.starts_with(rel)
    })
}

/// Remove everything under `dir` that a snapshot would carry.
fn clear_tree(dir: &Path, rel: &Path) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let child = rel.join(entry.file_name());
        if is_excluded(&child) {
            continue;
        }
        if entry.path().is_dir() && holds_excluded(&child) {
            clear_tree(&entry.path(), &child)?;
        } else {
            remove_any(&entry.path())?;
        }
    }
    Ok(())
}

/// Copy every child of `from` into `to`, skipping the excluded paths.
/// `fs::copy` clones on APFS, so an unchanged file costs no disk.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    copy_filtered(from, to, Path::new(""))
}

fn copy_filtered(from: &Path, to: &Path, rel: &Path) -> Result<()> {
    if !from.is_dir() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::copy(from, to)
            .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
        return Ok(());
    }
    fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let child = rel.join(&name);
        if is_excluded(&child) {
            continue;
        }
        copy_filtered(&entry.path(), &to.join(name), &child)?;
    }
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
