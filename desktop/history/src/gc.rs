//! Sweeping the history store's dead weight.
//!
//! `rotate_snapshots` decides which snapshots are *retained* by rewriting the
//! manifest's refs — and, until this module existed, did nothing else. The
//! snapshot record it dropped and the blobs only that record referenced stayed
//! on disk forever, so the store grew without bound: one real workspace reached
//! 2.1 GB across 42,000 files while retaining 138 snapshots.
//!
//! The sweep is deliberately *not* part of rotation. Rotation runs after every
//! recorded snapshot (about once a minute); enumerating tens of thousands of
//! blobs that often would cost far more than it reclaims. Instead the sweep runs
//! at most hourly, tracked in its own file so it never races the manifest write,
//! and on its own thread so a large first sweep cannot stall a save.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs, io,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::store::{blob_dir, gc_state_path, read_manifest, read_snapshot_record, snapshot_dir};
use crate::support::temp_suffix;

/// How often a full sweep is worth doing. Snapshots become unreferenced at the
/// rate rotation drops them, which is slow; the cost of finding them is
/// proportional to the whole store, which is not.
const SWEEP_INTERVAL_HOURS: i64 = 1;

/// How recently a file must have been touched to be spared regardless of what
/// the manifest says.
///
/// This is what makes the sweep safe to run *while* snapshots are being
/// recorded, without a lock: the sweep decides what is unreachable from a
/// manifest read at some instant, and anything a later snapshot creates or
/// re-uses (see `store::freshen`) is younger than that instant. It also spares
/// the temp file of an in-flight atomic write.
const RECENT_GRACE_HOURS: i64 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct GcState {
    #[serde(default)]
    pub(crate) last_sweep: Option<DateTime<Utc>>,
}

/// Set while a sweep thread is alive, so a burst of saves cannot start several.
static SWEEPING: AtomicBool = AtomicBool::new(false);

/// Starts a sweep on its own thread if one is due.
///
/// Returns without waiting: reclaiming a store that has never been swept can
/// mean unlinking tens of thousands of files, and the caller is the save path.
pub(crate) fn sweep_in_background_if_due(workspace_dir: &Path, now: DateTime<Utc>) {
    if !is_due(workspace_dir, now) {
        return;
    }
    if SWEEPING.swap(true, Ordering::SeqCst) {
        return;
    }
    let workspace_dir = workspace_dir.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("knotq-history-gc".into())
        .spawn(move || {
            if let Err(err) = sweep(&workspace_dir, now) {
                eprintln!("workspace history sweep failed: {err:#}");
            }
            SWEEPING.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        SWEEPING.store(false, Ordering::SeqCst);
    }
}

fn is_due(workspace_dir: &Path, now: DateTime<Utc>) -> bool {
    match read_gc_state(workspace_dir).last_sweep {
        // A clock that moved backwards must not defer the sweep indefinitely.
        Some(last) => now < last || now.signed_duration_since(last) >= Duration::hours(SWEEP_INTERVAL_HOURS),
        None => true,
    }
}

/// Deletes every snapshot record and blob the manifest no longer reaches.
pub(crate) fn sweep(workspace_dir: &Path, now: DateTime<Utc>) -> Result<SweepReport> {
    sweep_with_grace(workspace_dir, now, Duration::hours(RECENT_GRACE_HOURS))
}

/// `grace` is measured against the wall clock, not `now`: `now` is the logical
/// time the sweep cadence runs on, while file ages come from mtimes. Tests pass
/// a zero grace to make files written moments ago eligible.
fn sweep_with_grace(
    workspace_dir: &Path,
    now: DateTime<Utc>,
    grace: Duration,
) -> Result<SweepReport> {
    let manifest = read_manifest(workspace_dir)?;
    let retained_ids: HashSet<String> = manifest.refs.into_values().collect();

    let mut report = SweepReport::default();

    // The retained records are also where the set of live blobs comes from, so
    // read them first: if any is unreadable the blob sweep must not run, or a
    // blob that record still needs could be deleted.
    let mut retained_blobs: HashSet<String> = HashSet::new();
    let mut blob_set_is_complete = true;
    for id in &retained_ids {
        match read_snapshot_record(workspace_dir, id) {
            Ok(record) => retained_blobs.extend(record.entries.into_iter().filter_map(|e| e.blob)),
            Err(err) => {
                eprintln!("workspace history sweep skipping blobs: unreadable snapshot {id}: {err:#}");
                blob_set_is_complete = false;
            }
        }
    }

    report.records_removed = sweep_dir(&snapshot_dir(workspace_dir), grace, |name| {
        match name.strip_suffix(".json") {
            Some(id) => retained_ids.contains(id),
            None => false,
        }
    })?;

    if blob_set_is_complete {
        let blobs = blob_dir(workspace_dir);
        let shards = match fs::read_dir(&blobs) {
            Ok(shards) => shards,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                write_gc_state(workspace_dir, now)?;
                return Ok(report);
            }
            Err(err) => return Err(err).with_context(|| format!("read {}", blobs.display())),
        };
        for shard in shards {
            let shard = shard.with_context(|| format!("read entry in {}", blobs.display()))?;
            if !shard.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            report.blobs_removed +=
                sweep_dir(&shard.path(), grace, |name| retained_blobs.contains(name))?;
        }
    }

    write_gc_state(workspace_dir, now)?;
    Ok(report)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SweepReport {
    pub records_removed: usize,
    pub blobs_removed: usize,
}

/// Removes the files in `dir` that `keep` rejects, sparing any that were
/// touched within `grace`.
fn sweep_dir(dir: &Path, grace: Duration, keep: impl Fn(&str) -> bool) -> Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err).with_context(|| format!("read {}", dir.display())),
    };

    let mut removed = 0;
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", dir.display()))?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if !entry.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
            continue;
        }
        if keep(&name) {
            continue;
        }
        if was_touched_within(&entry, grace) {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            // Another process may have swept the same file; that is the
            // outcome we wanted either way.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("remove {}", entry.path().display()))
            }
        }
    }
    Ok(removed)
}

fn was_touched_within(entry: &fs::DirEntry, grace: Duration) -> bool {
    if grace <= Duration::zero() {
        return false;
    }
    let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
        // Unknown age: assume young, and let a later sweep take it.
        return true;
    };
    Utc::now().signed_duration_since(DateTime::<Utc>::from(modified)) < grace
}

fn read_gc_state(workspace_dir: &Path) -> GcState {
    // A missing or unreadable state file means "never swept", which is the
    // safe answer: the sweep itself is idempotent.
    fs::read(gc_state_path(workspace_dir))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn write_gc_state(workspace_dir: &Path, now: DateTime<Utc>) -> Result<()> {
    let path = gc_state_path(workspace_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let raw = serde_json::to_vec(&GcState {
        last_sweep: Some(now),
    })
    .context("serialize history gc state")?;
    let tmp = path.with_extension(format!("tmp-{}", temp_suffix()));
    fs::write(&tmp, raw).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("install {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::record_workspace_snapshot_at;
    use crate::store::{blob_path, read_manifest, store_blob};
    use crate::support::sha256_hex;
    use crate::{restore_workspace_snapshot, HISTORY_DIR, STORE_DIR};
    use std::{path::PathBuf, time::SystemTime};

    /// Most tests here write their fixture moments before sweeping, so they opt
    /// out of the grace that would otherwise spare the whole store.
    fn sweep_now(dir: &Path, now: DateTime<Utc>) -> Result<SweepReport> {
        sweep_with_grace(dir, now, Duration::zero())
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{}-{}", prefix, temp_suffix()))
    }

    fn count_files(dir: &Path) -> usize {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| {
                if entry.path().is_dir() {
                    count_files(&entry.path())
                } else {
                    1
                }
            })
            .sum()
    }

    /// Writes `count` snapshots far enough apart that rotation drops all but a
    /// handful, which is exactly the shape that grew a real store to 2.1 GB.
    fn workspace_with_rotated_snapshots(count: i64) -> (PathBuf, DateTime<Utc>) {
        let dir = unique_temp_dir("knotq-history-gc");
        fs::create_dir_all(&dir).unwrap();
        let start = Utc::now() - Duration::days(30);
        for i in 0..count {
            fs::write(dir.join("workspace.json"), format!("revision {i}")).unwrap();
            record_workspace_snapshot_at(&dir, start + Duration::minutes(i * 10)).unwrap();
        }
        (dir, start + Duration::minutes(count * 10))
    }

    #[test]
    fn sweeping_removes_records_and_blobs_rotation_dropped() {
        let (dir, now) = workspace_with_rotated_snapshots(40);
        let store = dir.join(HISTORY_DIR).join(STORE_DIR);
        let retained = read_manifest(&dir).unwrap().refs.len();
        let records_before = count_files(&store.join("snapshots"));
        let blobs_before = count_files(&store.join("blobs"));
        assert!(
            records_before > retained,
            "test needs rotation to have dropped snapshots ({records_before} records, \
             {retained} retained)"
        );

        let report = sweep_now(&dir, now).unwrap();

        assert_eq!(report.records_removed, records_before - retained);
        assert_eq!(count_files(&store.join("snapshots")), retained);
        assert!(
            count_files(&store.join("blobs")) < blobs_before,
            "orphaned blobs must be reclaimed too"
        );

        fs::remove_dir_all(dir).unwrap();
    }

    /// The whole point of the store is that a retained snapshot can still be
    /// restored after a sweep — so every blob it names must survive.
    #[test]
    fn every_retained_snapshot_is_still_restorable_after_a_sweep() {
        let (dir, now) = workspace_with_rotated_snapshots(40);
        sweep_now(&dir, now).unwrap();

        let refs = read_manifest(&dir).unwrap().refs;
        assert!(refs.len() > 1);
        for id in refs.values() {
            let record = read_snapshot_record(&dir, id).unwrap();
            for entry in &record.entries {
                if let Some(blob) = &entry.blob {
                    assert!(
                        blob_path(&dir, blob).exists(),
                        "sweep deleted blob {blob}, still referenced by snapshot {id}"
                    );
                }
            }
            restore_workspace_snapshot(&dir, id).unwrap();
        }

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sweeping_is_idempotent() {
        let (dir, now) = workspace_with_rotated_snapshots(20);
        sweep_now(&dir, now).unwrap();
        let second = sweep_now(&dir, now).unwrap();
        assert_eq!(second, SweepReport::default());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_sweep_is_due_once_an_hour_and_not_more() {
        let dir = unique_temp_dir("knotq-history-gc-due");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("workspace.json"), "one").unwrap();
        let now = Utc::now();

        assert!(is_due(&dir, now), "a store that never swept is due");
        write_gc_state(&dir, now).unwrap();
        assert!(!is_due(&dir, now + Duration::minutes(59)));
        assert!(is_due(&dir, now + Duration::hours(1)));
        // A clock jumping backwards must not defer sweeps indefinitely.
        assert!(is_due(&dir, now - Duration::hours(5)));

        fs::remove_dir_all(dir).unwrap();
    }

    /// An unreadable retained record means the live blob set is unknown, so the
    /// sweep must leave every blob alone rather than guess.
    #[test]
    fn an_unreadable_retained_record_spares_all_blobs() {
        let (dir, now) = workspace_with_rotated_snapshots(20);
        let store = dir.join(HISTORY_DIR).join(STORE_DIR);
        let victim = read_manifest(&dir)
            .unwrap()
            .refs
            .into_values()
            .next()
            .unwrap();
        fs::write(
            store.join("snapshots").join(format!("{victim}.json")),
            "not json",
        )
        .unwrap();
        let blobs_before = count_files(&store.join("blobs"));

        let report = sweep_now(&dir, now).unwrap();

        assert_eq!(report.blobs_removed, 0);
        assert_eq!(count_files(&store.join("blobs")), blobs_before);

        fs::remove_dir_all(dir).unwrap();
    }

    /// The grace is what lets the sweep run without a lock against snapshot
    /// recording: anything touched moments ago is spared, whether that is the
    /// temp file of an in-flight atomic write or a blob a snapshot that landed
    /// mid-sweep just claimed.
    #[test]
    fn recently_touched_files_are_spared() {
        let (dir, now) = workspace_with_rotated_snapshots(20);
        let store = dir.join(HISTORY_DIR).join(STORE_DIR);
        let in_flight = store.join("snapshots").join("abc.tmp-inflight");
        fs::write(&in_flight, "partial").unwrap();
        let files_before = count_files(&store);

        // Every file here was written seconds ago, so the real sweep — grace
        // and all — must leave the store exactly as it found it.
        let report = sweep(&dir, now).unwrap();

        assert_eq!(report, SweepReport::default());
        assert_eq!(count_files(&store), files_before);
        assert!(in_flight.exists());

        // Once they have aged out, the same call reclaims them.
        assert!(sweep_with_grace(&dir, now, Duration::zero()).unwrap() != SweepReport::default());
        assert!(!in_flight.exists());

        fs::remove_dir_all(dir).unwrap();
    }

    /// A snapshot that re-uses an existing blob must mark it as live, or the
    /// grace above protects nothing: the blob would still carry the mtime of
    /// whenever it was first stored.
    #[test]
    fn re_storing_an_existing_blob_marks_it_live() {
        let dir = unique_temp_dir("knotq-history-gc-freshen");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("workspace.json"), "one").unwrap();
        record_workspace_snapshot_at(&dir, Utc::now()).unwrap();

        let blob = sha256_hex(b"one");
        let path = blob_path(&dir, &blob);
        let stale = SystemTime::now() - std::time::Duration::from_secs(48 * 3600);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(stale)
            .unwrap();

        store_blob(&dir, &blob, b"one").unwrap();

        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(
            modified > stale,
            "re-storing an existing blob must refresh its mtime"
        );

        fs::remove_dir_all(dir).unwrap();
    }
}
