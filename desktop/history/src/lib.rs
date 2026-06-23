use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path, sync::atomic::AtomicU64};

mod capture;
mod retention;
mod store;
mod support;

use capture::{list_internal_snapshots, record_workspace_snapshot_at};
use retention::rotate_snapshots;
use store::{blob_path, history_store_exists, read_snapshot_record};
use support::{
    format_snapshot_label, remove_if_exists, resolve_snapshot_id, workspace_target_path,
};

pub(crate) const HISTORY_DIR: &str = ".knotq-history";
pub(crate) const STORE_DIR: &str = "v1";
pub(crate) const STORE_VERSION: u32 = 1;
pub(crate) const MANIFEST_FILE: &str = "manifest.json";
pub(crate) const BLOB_DIR: &str = "blobs";
pub(crate) const SNAPSHOT_DIR: &str = "snapshots";
const SNAPSHOT_REF_PREFIX: &str = "refs/knotq/snapshots";
pub(crate) const TRACKED_PATHS: &[&str] = &[
    "workspace.json",
    ".gitignore",
    "schemes",
    "daily_queue",
    "assets",
];

pub(crate) static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSnapshot {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub label: String,
}

#[derive(Clone, Debug)]
pub(crate) struct InternalSnapshot {
    pub(crate) id: String,
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) content_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RetentionBucket {
    pub(crate) tier: &'static str,
    pub(crate) start_epoch_secs: i64,
}

impl RetentionBucket {
    pub(crate) fn refname(&self) -> String {
        format!(
            "{SNAPSHOT_REF_PREFIX}/{}/{}",
            self.tier, self.start_epoch_secs
        )
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct StoreManifest {
    pub(crate) version: u32,
    pub(crate) refs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SnapshotRecord {
    pub(crate) version: u32,
    pub(crate) id: String,
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) content_hash: String,
    pub(crate) entries: Vec<SnapshotEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SnapshotEntry {
    pub(crate) path: String,
    pub(crate) kind: SnapshotEntryKind,
    pub(crate) blob: Option<String>,
    pub(crate) len: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SnapshotEntryKind {
    Dir,
    File,
}

pub fn record_workspace_snapshot(workspace_dir: &Path) -> Result<()> {
    record_workspace_snapshot_at(workspace_dir, Utc::now())
}

pub fn list_workspace_snapshots(workspace_dir: &Path) -> Result<Vec<WorkspaceSnapshot>> {
    if !history_store_exists(workspace_dir) {
        return Ok(Vec::new());
    }
    rotate_snapshots(workspace_dir, Utc::now())?;
    let mut snapshots = list_internal_snapshots(workspace_dir)?
        .into_iter()
        .map(|snapshot| WorkspaceSnapshot {
            id: snapshot.id,
            timestamp: snapshot.timestamp,
            label: format_snapshot_label(snapshot.timestamp),
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    snapshots.dedup_by(|a, b| a.id == b.id);
    Ok(snapshots)
}

pub fn restore_workspace_snapshot(workspace_dir: &Path, snapshot_id: &str) -> Result<()> {
    if !history_store_exists(workspace_dir) {
        bail!("workspace history has not been initialized");
    }
    let snapshot_id = resolve_snapshot_id(workspace_dir, snapshot_id)?;
    let snapshot = read_snapshot_record(workspace_dir, &snapshot_id)
        .with_context(|| format!("find history snapshot {snapshot_id}"))?;
    if !snapshot
        .entries
        .iter()
        .any(|entry| entry.kind == SnapshotEntryKind::File && entry.path == "workspace.json")
    {
        bail!("history snapshot {snapshot_id} does not contain workspace.json");
    }

    for path in TRACKED_PATHS {
        remove_if_exists(&workspace_dir.join(path))?;
    }

    let mut entries = snapshot.entries;
    entries.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(&b.kind)));

    for entry in entries
        .iter()
        .filter(|entry| entry.kind == SnapshotEntryKind::Dir)
    {
        fs::create_dir_all(workspace_target_path(workspace_dir, &entry.path)?)
            .with_context(|| format!("restore directory {}", entry.path))?;
    }

    for entry in entries
        .iter()
        .filter(|entry| entry.kind == SnapshotEntryKind::File)
    {
        let target = workspace_target_path(workspace_dir, &entry.path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create restore parent {}", parent.display()))?;
        }
        let blob = entry
            .blob
            .as_deref()
            .ok_or_else(|| anyhow!("snapshot file entry {} has no blob", entry.path))?;
        fs::copy(blob_path(workspace_dir, blob), &target)
            .with_context(|| format!("restore file {}", entry.path))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::retention_bucket;
    use crate::support::temp_suffix;
    use chrono::{Duration, TimeZone};
    use std::path::PathBuf;

    #[test]
    fn retention_bucket_uses_requested_cadence() {
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 18, 17, 0).unwrap();

        assert_eq!(
            retention_bucket(now - Duration::minutes(55), now)
                .unwrap()
                .tier,
            "m5"
        );
        assert_eq!(
            retention_bucket(now - Duration::hours(47), now)
                .unwrap()
                .tier,
            "h1"
        );
        assert_eq!(
            retention_bucket(now - Duration::days(6), now).unwrap().tier,
            "h4"
        );
        assert_eq!(
            retention_bucket(now - Duration::days(300), now)
                .unwrap()
                .tier,
            "d1"
        );
        assert!(retention_bucket(now - Duration::days(366), now).is_none());
    }

    #[test]
    fn record_list_and_restore_snapshots_without_git() {
        let workspace_dir = unique_temp_dir("knotq-history-restore");
        fs::create_dir_all(workspace_dir.join("schemes").join("Project")).unwrap();
        fs::create_dir_all(workspace_dir.join("daily_queue")).unwrap();
        fs::write(workspace_dir.join(".gitignore"), ".knotq-history/\n").unwrap();
        fs::write(workspace_dir.join("workspace.json"), "one").unwrap();
        fs::write(
            workspace_dir
                .join("schemes")
                .join("Project")
                .join("Task.knotq"),
            "first",
        )
        .unwrap();

        let first_time = Utc
            .timestamp_opt(Utc::now().timestamp() - 10 * 60, 0)
            .unwrap();
        record_workspace_snapshot_at(&workspace_dir, first_time).unwrap();

        fs::write(workspace_dir.join("workspace.json"), "two").unwrap();
        fs::write(
            workspace_dir
                .join("schemes")
                .join("Project")
                .join("Task.knotq"),
            "second",
        )
        .unwrap();
        record_workspace_snapshot_at(&workspace_dir, Utc::now()).unwrap();

        let snapshots = list_workspace_snapshots(&workspace_dir).unwrap();
        assert!(snapshots.len() >= 2);
        let first = snapshots
            .iter()
            .find(|snapshot| snapshot.timestamp == first_time)
            .unwrap();
        restore_workspace_snapshot(&workspace_dir, &first.id).unwrap();

        assert_eq!(
            fs::read_to_string(workspace_dir.join("workspace.json")).unwrap(),
            "one"
        );
        assert_eq!(
            fs::read_to_string(
                workspace_dir
                    .join("schemes")
                    .join("Project")
                    .join("Task.knotq")
            )
            .unwrap(),
            "first"
        );
        assert!(workspace_dir
            .join(HISTORY_DIR)
            .join(STORE_DIR)
            .join(MANIFEST_FILE)
            .exists());

        fs::remove_dir_all(workspace_dir).unwrap();
    }

    #[test]
    fn unchanged_content_does_not_create_duplicate_snapshot() {
        let workspace_dir = unique_temp_dir("knotq-history-dedupe");
        fs::create_dir_all(&workspace_dir).unwrap();
        fs::write(workspace_dir.join("workspace.json"), "one").unwrap();
        let first_time = Utc.with_ymd_and_hms(2026, 5, 20, 18, 0, 0).unwrap();
        let second_time = first_time + Duration::minutes(5);

        record_workspace_snapshot_at(&workspace_dir, first_time).unwrap();
        record_workspace_snapshot_at(&workspace_dir, second_time).unwrap();

        let snapshots = list_workspace_snapshots(&workspace_dir).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].timestamp, first_time);

        fs::remove_dir_all(workspace_dir).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{}-{}", prefix, temp_suffix()))
    }
}
