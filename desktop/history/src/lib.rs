use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const HISTORY_DIR: &str = ".knotq-history";
const STORE_DIR: &str = "v1";
const STORE_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";
const BLOB_DIR: &str = "blobs";
const SNAPSHOT_DIR: &str = "snapshots";
const SNAPSHOT_REF_PREFIX: &str = "refs/knotq/snapshots";
const TRACKED_PATHS: &[&str] = &[
    "workspace.json",
    ".gitignore",
    "schemes",
    "daily_queue",
    "assets",
];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSnapshot {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub label: String,
}

#[derive(Clone, Debug)]
struct InternalSnapshot {
    id: String,
    timestamp: DateTime<Utc>,
    content_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RetentionBucket {
    tier: &'static str,
    start_epoch_secs: i64,
}

impl RetentionBucket {
    fn refname(&self) -> String {
        format!(
            "{SNAPSHOT_REF_PREFIX}/{}/{}",
            self.tier, self.start_epoch_secs
        )
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct StoreManifest {
    version: u32,
    refs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SnapshotRecord {
    version: u32,
    id: String,
    timestamp: DateTime<Utc>,
    content_hash: String,
    entries: Vec<SnapshotEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct SnapshotEntry {
    path: String,
    kind: SnapshotEntryKind,
    blob: Option<String>,
    len: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SnapshotEntryKind {
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

fn record_workspace_snapshot_at(workspace_dir: &Path, now: DateTime<Utc>) -> Result<()> {
    ensure_history_store(workspace_dir)?;
    let snapshot = build_snapshot_record(workspace_dir, now)?;
    if snapshot.entries.is_empty() {
        bail!("workspace history has no files to snapshot");
    }
    if !snapshot
        .entries
        .iter()
        .any(|entry| entry.kind == SnapshotEntryKind::File && entry.path == "workspace.json")
    {
        bail!("workspace history requires workspace.json");
    }
    if let Some(latest) = latest_snapshot(workspace_dir)? {
        if latest.content_hash == snapshot.content_hash {
            return Ok(());
        }
    }

    let bucket = retention_bucket(now, now)
        .ok_or_else(|| anyhow!("new history snapshots must be within retention"))?;
    write_snapshot_record(workspace_dir, &snapshot)?;
    update_ref(workspace_dir, &bucket.refname(), &snapshot.id)?;
    rotate_snapshots(workspace_dir, now)?;
    Ok(())
}

fn ensure_history_store(workspace_dir: &Path) -> Result<()> {
    fs::create_dir_all(workspace_dir)
        .with_context(|| format!("create {}", workspace_dir.display()))?;
    fs::create_dir_all(blob_dir(workspace_dir))
        .with_context(|| format!("create {}", blob_dir(workspace_dir).display()))?;
    fs::create_dir_all(snapshot_dir(workspace_dir))
        .with_context(|| format!("create {}", snapshot_dir(workspace_dir).display()))?;
    if !manifest_path(workspace_dir).exists() {
        write_manifest(
            workspace_dir,
            &StoreManifest {
                version: STORE_VERSION,
                refs: BTreeMap::new(),
            },
        )?;
    }
    Ok(())
}

fn history_store_exists(workspace_dir: &Path) -> bool {
    manifest_path(workspace_dir).exists()
}

fn history_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(HISTORY_DIR)
}

fn store_dir(workspace_dir: &Path) -> PathBuf {
    history_dir(workspace_dir).join(STORE_DIR)
}

fn blob_dir(workspace_dir: &Path) -> PathBuf {
    store_dir(workspace_dir).join(BLOB_DIR)
}

fn snapshot_dir(workspace_dir: &Path) -> PathBuf {
    store_dir(workspace_dir).join(SNAPSHOT_DIR)
}

fn manifest_path(workspace_dir: &Path) -> PathBuf {
    store_dir(workspace_dir).join(MANIFEST_FILE)
}

fn build_snapshot_record(workspace_dir: &Path, timestamp: DateTime<Utc>) -> Result<SnapshotRecord> {
    let mut entries = Vec::new();
    for path in TRACKED_PATHS {
        collect_snapshot_entries(workspace_dir, Path::new(path), &mut entries)?;
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(&b.kind)));
    entries.dedup_by(|a, b| a.path == b.path && a.kind == b.kind);

    let content_hash = snapshot_content_hash(&entries);
    let id = snapshot_id(timestamp, &content_hash);
    Ok(SnapshotRecord {
        version: STORE_VERSION,
        id,
        timestamp,
        content_hash,
        entries,
    })
}

fn collect_snapshot_entries(
    workspace_dir: &Path,
    relative: &Path,
    entries: &mut Vec<SnapshotEntry>,
) -> Result<()> {
    validate_relative_path(relative)?;
    let absolute = workspace_dir.join(relative);
    let metadata = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("stat {}", absolute.display())),
    };

    if metadata.is_dir() {
        entries.push(SnapshotEntry {
            path: stored_path(relative)?,
            kind: SnapshotEntryKind::Dir,
            blob: None,
            len: 0,
        });

        let mut children = fs::read_dir(&absolute)
            .with_context(|| format!("read directory {}", absolute.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("read directory entry {}", absolute.display()))?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            collect_snapshot_entries(workspace_dir, &relative.join(child.file_name()), entries)?;
        }
        return Ok(());
    }

    if !metadata.is_file() {
        return Ok(());
    }

    let bytes = fs::read(&absolute).with_context(|| format!("read {}", absolute.display()))?;
    let blob = sha256_hex(&bytes);
    store_blob(workspace_dir, &blob, &bytes)?;
    entries.push(SnapshotEntry {
        path: stored_path(relative)?,
        kind: SnapshotEntryKind::File,
        blob: Some(blob),
        len: bytes.len() as u64,
    });
    Ok(())
}

fn store_blob(workspace_dir: &Path, blob: &str, bytes: &[u8]) -> Result<()> {
    validate_blob_id(blob)?;
    let path = blob_path(workspace_dir, blob);
    if path.exists() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("blob path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let tmp = path.with_extension(format!("tmp-{}", temp_suffix()));
    fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("install blob {}", path.display()))?;
    Ok(())
}

fn blob_path(workspace_dir: &Path, blob: &str) -> PathBuf {
    let prefix = &blob[..2];
    blob_dir(workspace_dir).join(prefix).join(blob)
}

fn snapshot_content_hash(entries: &[SnapshotEntry]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq-history-content-v1\0");
    for entry in entries {
        hasher.update(entry.path.as_bytes());
        hasher.update([0]);
        hasher.update(match entry.kind {
            SnapshotEntryKind::Dir => b"dir".as_slice(),
            SnapshotEntryKind::File => b"file".as_slice(),
        });
        hasher.update([0]);
        if let Some(blob) = &entry.blob {
            hasher.update(blob.as_bytes());
        }
        hasher.update([0]);
        hasher.update(entry.len.to_be_bytes());
        hasher.update([0]);
    }
    hex_digest(hasher.finalize())
}

fn snapshot_id(timestamp: DateTime<Utc>, content_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq-history-snapshot-v1\0");
    hasher.update(
        timestamp
            .to_rfc3339_opts(SecondsFormat::Nanos, true)
            .as_bytes(),
    );
    hasher.update([0]);
    hasher.update(content_hash.as_bytes());
    hex_digest(hasher.finalize())
}

fn latest_snapshot(workspace_dir: &Path) -> Result<Option<InternalSnapshot>> {
    Ok(list_internal_snapshots(workspace_dir)?
        .into_iter()
        .max_by_key(|snapshot| snapshot.timestamp))
}

fn list_internal_snapshots(workspace_dir: &Path) -> Result<Vec<InternalSnapshot>> {
    if !history_store_exists(workspace_dir) {
        return Ok(Vec::new());
    }
    let manifest = read_manifest(workspace_dir)?;
    let mut snapshots = Vec::new();
    for (refname, id) in manifest.refs {
        let snapshot = read_snapshot_record(workspace_dir, &id)
            .with_context(|| format!("read history snapshot {id} for {refname}"))?;
        snapshots.push(InternalSnapshot {
            id: snapshot.id,
            timestamp: snapshot.timestamp,
            content_hash: snapshot.content_hash,
        });
    }
    Ok(snapshots)
}

fn rotate_snapshots(workspace_dir: &Path, now: DateTime<Utc>) -> Result<()> {
    let snapshots = list_internal_snapshots(workspace_dir)?;
    let mut refs = BTreeMap::<String, String>::new();
    let mut keep_by_ref = BTreeMap::<String, InternalSnapshot>::new();
    for snapshot in &snapshots {
        let Some(bucket) = retention_bucket(snapshot.timestamp, now) else {
            continue;
        };
        let refname = bucket.refname();
        let replace = keep_by_ref
            .get(&refname)
            .map(|existing| snapshot.timestamp > existing.timestamp)
            .unwrap_or(true);
        if replace {
            keep_by_ref.insert(refname, snapshot.clone());
        }
    }

    for (refname, snapshot) in keep_by_ref {
        refs.insert(refname, snapshot.id);
    }
    write_manifest(
        workspace_dir,
        &StoreManifest {
            version: STORE_VERSION,
            refs,
        },
    )?;
    prune_unreferenced_objects(workspace_dir)?;
    Ok(())
}

fn retention_bucket(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> Option<RetentionBucket> {
    let age = now.signed_duration_since(timestamp);
    let (tier, step_secs) = if age <= Duration::hours(1) {
        ("m5", 5 * 60)
    } else if age <= Duration::hours(48) {
        ("h1", 60 * 60)
    } else if age <= Duration::days(7) {
        ("h4", 4 * 60 * 60)
    } else if age <= Duration::days(365) {
        ("d1", 24 * 60 * 60)
    } else {
        return None;
    };
    Some(RetentionBucket {
        tier,
        start_epoch_secs: floor_epoch(timestamp.timestamp(), step_secs),
    })
}

fn floor_epoch(timestamp: i64, step_secs: i64) -> i64 {
    timestamp.div_euclid(step_secs) * step_secs
}

fn update_ref(workspace_dir: &Path, refname: &str, snapshot: &str) -> Result<()> {
    validate_snapshot_id(snapshot)?;
    let mut manifest = read_manifest(workspace_dir)?;
    manifest
        .refs
        .insert(refname.to_string(), snapshot.to_string());
    write_manifest(workspace_dir, &manifest)
        .with_context(|| format!("update history ref {refname}"))?;
    Ok(())
}

fn read_manifest(workspace_dir: &Path) -> Result<StoreManifest> {
    let path = manifest_path(workspace_dir);
    let raw = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let manifest: StoreManifest =
        serde_json::from_slice(&raw).with_context(|| format!("parse {}", path.display()))?;
    if manifest.version != STORE_VERSION {
        bail!("unsupported history store version {}", manifest.version);
    }
    for id in manifest.refs.values() {
        validate_snapshot_id(id)?;
    }
    Ok(manifest)
}

fn write_manifest(workspace_dir: &Path, manifest: &StoreManifest) -> Result<()> {
    let path = manifest_path(workspace_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(manifest).context("serialize history manifest")?;
    write_json_atomic(&path, &raw)
}

fn read_snapshot_record(workspace_dir: &Path, id: &str) -> Result<SnapshotRecord> {
    validate_snapshot_id(id)?;
    let path = snapshot_path(workspace_dir, id);
    let raw = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let snapshot: SnapshotRecord =
        serde_json::from_slice(&raw).with_context(|| format!("parse {}", path.display()))?;
    if snapshot.version != STORE_VERSION {
        bail!("unsupported history snapshot version {}", snapshot.version);
    }
    if snapshot.id != id {
        bail!("history snapshot id mismatch");
    }
    validate_snapshot_record(&snapshot)?;
    Ok(snapshot)
}

fn write_snapshot_record(workspace_dir: &Path, snapshot: &SnapshotRecord) -> Result<()> {
    validate_snapshot_record(snapshot)?;
    let path = snapshot_path(workspace_dir, &snapshot.id);
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(snapshot).context("serialize history snapshot")?;
    write_json_atomic(&path, &raw)
}

fn snapshot_path(workspace_dir: &Path, id: &str) -> PathBuf {
    snapshot_dir(workspace_dir).join(format!("{id}.json"))
}

fn write_json_atomic(path: &Path, raw: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", temp_suffix()));
    fs::write(&tmp, raw).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("install {}", path.display()))?;
    Ok(())
}

fn validate_snapshot_record(snapshot: &SnapshotRecord) -> Result<()> {
    validate_snapshot_id(&snapshot.id)?;
    validate_blob_id(&snapshot.content_hash)?;
    for entry in &snapshot.entries {
        validate_relative_path(Path::new(&entry.path))?;
        match entry.kind {
            SnapshotEntryKind::Dir => {
                if entry.blob.is_some() || entry.len != 0 {
                    bail!("history directory entry {} has file data", entry.path);
                }
            }
            SnapshotEntryKind::File => {
                let blob = entry
                    .blob
                    .as_deref()
                    .ok_or_else(|| anyhow!("history file entry {} has no blob", entry.path))?;
                validate_blob_id(blob)?;
            }
        }
    }
    Ok(())
}

fn resolve_snapshot_id(workspace_dir: &Path, id: &str) -> Result<String> {
    validate_snapshot_id(id)?;
    if id.len() == 64 {
        return Ok(id.to_string());
    }

    let mut matches = Vec::new();
    if snapshot_dir(workspace_dir).exists() {
        for entry in fs::read_dir(snapshot_dir(workspace_dir))
            .with_context(|| format!("read {}", snapshot_dir(workspace_dir).display()))?
        {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            let Some(candidate) = name.strip_suffix(".json") else {
                continue;
            };
            if candidate.starts_with(id) {
                matches.push(candidate.to_string());
            }
        }
    }

    match matches.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!("history snapshot {id} not found"),
        _ => bail!("history snapshot id {id} is ambiguous"),
    }
}

fn prune_unreferenced_objects(workspace_dir: &Path) -> Result<()> {
    let manifest = read_manifest(workspace_dir)?;
    let live_snapshots = manifest.refs.values().cloned().collect::<BTreeSet<_>>();
    let mut live_blobs = HashSet::new();
    for id in &live_snapshots {
        let snapshot = read_snapshot_record(workspace_dir, id)?;
        for entry in snapshot.entries {
            if let Some(blob) = entry.blob {
                live_blobs.insert(blob);
            }
        }
    }

    if snapshot_dir(workspace_dir).exists() {
        for entry in fs::read_dir(snapshot_dir(workspace_dir))
            .with_context(|| format!("read {}", snapshot_dir(workspace_dir).display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(ToOwned::to_owned)
            else {
                continue;
            };
            if path.extension().and_then(|ext| ext.to_str()) == Some("json")
                && !live_snapshots.contains(&id)
            {
                remove_if_exists(&path)?;
            }
        }
    }

    if blob_dir(workspace_dir).exists() {
        for prefix in fs::read_dir(blob_dir(workspace_dir))
            .with_context(|| format!("read {}", blob_dir(workspace_dir).display()))?
        {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for blob in fs::read_dir(prefix.path())? {
                let blob = blob?;
                let path = blob.path();
                let Some(id) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(ToOwned::to_owned)
                else {
                    continue;
                };
                if validate_blob_id(&id).is_ok() && !live_blobs.contains(&id) {
                    remove_if_exists(&path)?;
                }
            }
            let _ = fs::remove_dir(prefix.path());
        }
    }

    Ok(())
}

fn workspace_target_path(workspace_dir: &Path, stored: &str) -> Result<PathBuf> {
    validate_relative_path(Path::new(stored))?;
    Ok(workspace_dir.join(stored))
}

fn stored_path(relative: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| anyhow!("history paths must be valid UTF-8"))?;
                parts.push(part.to_string());
            }
            _ => bail!("invalid history path {}", relative.display()),
        }
    }
    if parts.is_empty() {
        bail!("history path cannot be empty");
    }
    Ok(parts.join("/"))
}

fn validate_relative_path(path: &Path) -> Result<()> {
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            _ => bail!("invalid history path {}", path.display()),
        }
    }
    if !has_component {
        bail!("history path cannot be empty");
    }
    Ok(())
}

fn validate_snapshot_id(id: &str) -> Result<()> {
    if id.len() < 7 || id.len() > 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid history snapshot id");
    }
    Ok(())
}

fn validate_blob_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid history blob id");
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("remove {}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("stat {}", path.display())),
    }
}

fn format_snapshot_label(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn temp_suffix() -> String {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{counter}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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
