use anyhow::{anyhow, bail, Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::support::{temp_suffix, validate_blob_id, validate_relative_path, validate_snapshot_id};
use crate::{
    SnapshotEntryKind, SnapshotRecord, StoreManifest, BLOB_DIR, HISTORY_DIR, MANIFEST_FILE,
    SNAPSHOT_DIR, STORE_DIR, STORE_VERSION,
};

pub(crate) fn ensure_history_store(workspace_dir: &Path) -> Result<()> {
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

pub(crate) fn history_store_exists(workspace_dir: &Path) -> bool {
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

pub(crate) fn snapshot_dir(workspace_dir: &Path) -> PathBuf {
    store_dir(workspace_dir).join(SNAPSHOT_DIR)
}

fn manifest_path(workspace_dir: &Path) -> PathBuf {
    store_dir(workspace_dir).join(MANIFEST_FILE)
}

pub(crate) fn store_blob(workspace_dir: &Path, blob: &str, bytes: &[u8]) -> Result<()> {
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

pub(crate) fn blob_path(workspace_dir: &Path, blob: &str) -> PathBuf {
    let prefix = &blob[..2];
    blob_dir(workspace_dir).join(prefix).join(blob)
}

pub(crate) fn update_ref(workspace_dir: &Path, refname: &str, snapshot: &str) -> Result<()> {
    validate_snapshot_id(snapshot)?;
    let mut manifest = read_manifest(workspace_dir)?;
    manifest
        .refs
        .insert(refname.to_string(), snapshot.to_string());
    write_manifest(workspace_dir, &manifest)
        .with_context(|| format!("update history ref {refname}"))?;
    Ok(())
}

pub(crate) fn read_manifest(workspace_dir: &Path) -> Result<StoreManifest> {
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

pub(crate) fn write_manifest(workspace_dir: &Path, manifest: &StoreManifest) -> Result<()> {
    let path = manifest_path(workspace_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(manifest).context("serialize history manifest")?;
    write_json_atomic(&path, &raw)
}

pub(crate) fn read_snapshot_record(workspace_dir: &Path, id: &str) -> Result<SnapshotRecord> {
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

pub(crate) fn write_snapshot_record(workspace_dir: &Path, snapshot: &SnapshotRecord) -> Result<()> {
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
