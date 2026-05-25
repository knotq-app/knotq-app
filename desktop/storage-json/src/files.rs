use anyhow::{anyhow, Context, Result};
use chrono::NaiveDate;
use knotq_model::{Scheme, SchemeId, Workspace};
use std::collections::HashSet;
use std::io;
use std::{fs, path::Path};

use crate::{
    cal_index::daily_queue_calendar_index_matches_range,
    options::WorkspaceLoadOptions,
    paths::daily_queue_file_path,
    schema::{WorkspaceEnvelope, WorkspaceIndex},
    scheme_file::{
        ensure_scheme_directories, prune_removed_daily_queue_files, prune_removed_scheme_files,
        read_daily_queue_file, read_existing_daily_queue_index, write_daily_backup,
        write_daily_queue_file, write_scheme_file,
    },
};

pub(crate) const SCHEMA_VERSION: u32 = 2;
pub(crate) const SETTINGS_SCHEMA_VERSION: u32 = 1;
const WORKSPACE_GITIGNORE: &str =
    "# KnotQ local files\n.knotq-history/\nbackups/\n*.tmp\n.DS_Store\n";

pub fn load_workspace(path: &Path) -> Result<Option<Workspace>> {
    load_workspace_with_options(path, WorkspaceLoadOptions::all())
}

pub fn load_workspace_with_options(
    path: &Path,
    options: WorkspaceLoadOptions,
) -> Result<Option<Workspace>> {
    let Some(env) = read_workspace_envelope(path)? else {
        return Ok(None);
    };
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    env.workspace
        .into_workspace_with_options(base_dir, options)
        .map(Some)
}

pub fn load_daily_queue_scheme(path: &Path, date: NaiveDate) -> Result<Option<Scheme>> {
    let Some(env) = read_workspace_envelope(path)? else {
        return Ok(None);
    };
    let Some(entry) = env
        .workspace
        .daily_queue
        .into_iter()
        .find(|entry| entry.date == date)
    else {
        return Ok(None);
    };
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file = match read_daily_queue_file(base_dir, date, entry.scheme.id) {
        Ok(file) => file,
        Err(err) if is_not_found(&err) => return Ok(None),
        Err(err) => return Err(err),
    };
    if file.id != entry.scheme.id {
        return Err(anyhow!(
            "daily queue file {} contains id {}",
            daily_queue_file_path(base_dir, date).display(),
            file.id
        ));
    }
    Ok(Some(crate::scheme_file::scheme_from_index(
        entry.scheme,
        file.items,
    )))
}

pub fn load_daily_queue_schemes_for_calendar_range(
    path: &Path,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<(NaiveDate, Scheme)>> {
    let Some(env) = read_workspace_envelope(path)? else {
        return Ok(Vec::new());
    };
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut schemes = Vec::new();
    for entry in env.workspace.daily_queue {
        if !daily_queue_calendar_index_matches_range(
            &entry.scheme.calendar_index,
            Some(start),
            Some(end),
        ) {
            continue;
        }
        let file = match read_daily_queue_file(base_dir, entry.date, entry.scheme.id) {
            Ok(file) => file,
            Err(err) => {
                if is_not_found(&err) {
                    continue;
                }
                return Err(err);
            }
        };
        if file.id != entry.scheme.id {
            return Err(anyhow!(
                "daily queue file {} contains id {}",
                daily_queue_file_path(base_dir, entry.date).display(),
                file.id
            ));
        }
        schemes.push((
            entry.date,
            crate::scheme_file::scheme_from_index(entry.scheme, file.items),
        ));
    }
    Ok(schemes)
}

pub fn save_workspace(path: &Path, workspace: &Workspace) -> Result<()> {
    let mut workspace = workspace.clone();
    workspace.ensure_sync_metadata();
    let workspace = &workspace;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(base_dir).with_context(|| format!("create {}", base_dir.display()))?;
    ensure_workspace_gitignore(base_dir)?;
    let schemes_dir = base_dir.join("schemes");
    fs::create_dir_all(&schemes_dir)
        .with_context(|| format!("create {}", schemes_dir.display()))?;
    ensure_scheme_directories(base_dir, workspace)?;
    let daily_queue_dir = base_dir.join("daily_queue");
    fs::create_dir_all(&daily_queue_dir)
        .with_context(|| format!("create {}", daily_queue_dir.display()))?;

    let daily_ids: HashSet<SchemeId> = workspace.daily_queue.values().copied().collect();

    for scheme in workspace
        .schemes
        .values()
        .filter(|scheme| !daily_ids.contains(&scheme.id))
    {
        write_scheme_file(base_dir, workspace, scheme)
            .with_context(|| format!("write scheme {}", scheme.id))?;
    }
    prune_removed_scheme_files(base_dir, workspace)?;

    for (date, scheme_id) in &workspace.daily_queue {
        if let Some(scheme) = workspace.schemes.get(scheme_id) {
            write_daily_queue_file(base_dir, *date, scheme)
                .with_context(|| format!("write daily queue {}", date))?;
        }
    }
    prune_removed_daily_queue_files(&daily_queue_dir, workspace)?;

    let existing_daily_queue = read_existing_daily_queue_index(path)?;
    let env = WorkspaceEnvelope {
        version: SCHEMA_VERSION,
        workspace: WorkspaceIndex::from_workspace_preserving(workspace, existing_daily_queue),
    };
    let json = serde_json::to_string_pretty(&env)?;
    write_atomic(path, json.as_bytes())?;
    write_daily_backup(base_dir, &json, workspace);
    record_history_snapshot(base_dir);

    Ok(())
}

/// Save only the specified dirty schemes and the workspace index.
/// Skips the daily backup and file pruning for speed; those are done
/// on full saves (e.g. at app shutdown).
pub fn save_workspace_incremental(
    path: &Path,
    workspace: &Workspace,
    dirty_scheme_ids: &HashSet<SchemeId>,
) -> Result<()> {
    let mut workspace = workspace.clone();
    workspace.ensure_sync_metadata();
    let workspace = &workspace;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(base_dir).with_context(|| format!("create {}", base_dir.display()))?;
    ensure_workspace_gitignore(base_dir)?;
    let schemes_dir = base_dir.join("schemes");
    fs::create_dir_all(&schemes_dir)
        .with_context(|| format!("create {}", schemes_dir.display()))?;
    ensure_scheme_directories(base_dir, workspace)?;
    let daily_queue_dir = base_dir.join("daily_queue");
    fs::create_dir_all(&daily_queue_dir)
        .with_context(|| format!("create {}", daily_queue_dir.display()))?;

    let daily_ids: HashSet<SchemeId> = workspace.daily_queue.values().copied().collect();

    // Write only dirty scheme files.
    for scheme_id in dirty_scheme_ids {
        if let Some(scheme) = workspace.schemes.get(scheme_id) {
            if daily_ids.contains(scheme_id) {
                if let Some(date) = workspace.daily_queue.iter().find_map(|(date, id)| {
                    if id == scheme_id {
                        Some(*date)
                    } else {
                        None
                    }
                }) {
                    write_daily_queue_file(base_dir, date, scheme)
                        .with_context(|| format!("write daily queue {}", date))?;
                }
            } else {
                write_scheme_file(base_dir, workspace, scheme)
                    .with_context(|| format!("write scheme {}", scheme.id))?;
            }
        }
    }
    prune_removed_scheme_files(base_dir, workspace)?;

    // Always rewrite the workspace index (it's small and metadata may have changed).
    let existing_daily_queue = read_existing_daily_queue_index(path)?;
    let env = WorkspaceEnvelope {
        version: SCHEMA_VERSION,
        workspace: WorkspaceIndex::from_workspace_preserving(workspace, existing_daily_queue),
    };
    let json = serde_json::to_string_pretty(&env)?;
    write_atomic(path, json.as_bytes())?;
    record_history_snapshot(base_dir);

    Ok(())
}

pub(crate) fn read_workspace_envelope(path: &Path) -> Result<Option<WorkspaceEnvelope>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let env: WorkspaceEnvelope = serde_json::from_str(&raw).context("parse workspace index")?;
    validate_workspace_version(env.version)?;
    Ok(Some(env))
}

pub(crate) fn validate_workspace_version(version: u32) -> Result<()> {
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(anyhow!(
            "unsupported workspace schema version {}, expected 1..={}",
            version,
            SCHEMA_VERSION
        ));
    }
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.tmp")),
        None => path.with_extension("tmp"),
    };
    fs::write(&tmp, contents).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))?;
    Ok(())
}

fn ensure_workspace_gitignore(base_dir: &Path) -> Result<()> {
    let path = base_dir.join(".gitignore");
    if !path.exists() {
        return write_atomic(&path, WORKSPACE_GITIGNORE.as_bytes());
    }
    let existing = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut updated = existing.clone();
    for line in [".knotq-history/", "backups/", "*.tmp", ".DS_Store"] {
        if !existing.lines().any(|existing_line| existing_line == line) {
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(line);
            updated.push('\n');
        }
    }
    if updated == existing {
        return Ok(());
    }
    write_atomic(&path, updated.as_bytes())
}

fn record_history_snapshot(base_dir: &Path) {
    if let Err(err) = knotq_history::record_workspace_snapshot(base_dir) {
        eprintln!("workspace history snapshot failed: {err:#}");
    }
}

fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|err| err.kind() == io::ErrorKind::NotFound)
    })
}

pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}
