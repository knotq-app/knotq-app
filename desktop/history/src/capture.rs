use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};

use crate::retention::{retention_bucket, rotate_snapshots};
use crate::store::{
    ensure_history_store, history_store_exists, read_manifest, read_snapshot_record, store_blob,
    update_ref, write_snapshot_record,
};
use crate::support::{hex_digest, sha256_hex, stored_path, validate_relative_path};
use crate::{
    InternalSnapshot, SnapshotEntry, SnapshotEntryKind, SnapshotRecord, STORE_VERSION,
    TRACKED_PATHS,
};

pub(crate) fn record_workspace_snapshot_at(workspace_dir: &Path, now: DateTime<Utc>) -> Result<()> {
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

pub(crate) fn list_internal_snapshots(workspace_dir: &Path) -> Result<Vec<InternalSnapshot>> {
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
