use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Local, Utc};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::atomic::Ordering,
};

use crate::store::snapshot_dir;
use crate::TEMP_COUNTER;

pub(crate) fn resolve_snapshot_id(workspace_dir: &Path, id: &str) -> Result<String> {
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

pub(crate) fn workspace_target_path(workspace_dir: &Path, stored: &str) -> Result<PathBuf> {
    validate_relative_path(Path::new(stored))?;
    Ok(workspace_dir.join(stored))
}

pub(crate) fn stored_path(relative: &Path) -> Result<String> {
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

pub(crate) fn validate_relative_path(path: &Path) -> Result<()> {
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

pub(crate) fn validate_snapshot_id(id: &str) -> Result<()> {
    if id.len() < 7 || id.len() > 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid history snapshot id");
    }
    Ok(())
}

pub(crate) fn validate_blob_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid history blob id");
    }
    Ok(())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

pub(crate) fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("remove {}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("stat {}", path.display())),
    }
}

pub(crate) fn format_snapshot_label(timestamp: DateTime<Utc>) -> String {
    timestamp
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M %Z")
        .to_string()
}

pub(crate) fn temp_suffix() -> String {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{counter}")
}
