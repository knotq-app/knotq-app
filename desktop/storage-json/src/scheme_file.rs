use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, NaiveDate};
use knotq_model::{Folder, FolderId, Scheme, SchemeId, Workspace};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    files::write_atomic,
    paths::schemes_dir,
    schema::{DailyQueueIndexEntry, SchemeIndex},
    scheme_markdown::decode_scheme_file,
    scheme_xml::{decode_scheme_xml, encode_scheme_xml},
};

const SCHEME_EXT: &str = "knotq";

#[derive(Serialize, Deserialize)]
pub(crate) struct SchemeFile {
    pub(crate) id: SchemeId,
    pub(crate) items: Vec<knotq_model::Item>,
}

pub(crate) fn scheme_from_index(index: SchemeIndex, items: Vec<knotq_model::Item>) -> Scheme {
    let mut scheme = Scheme {
        id: index.id,
        name: index.name,
        color_index: index.color_index,
        gsync: index.gsync,
        source: index.source,
        items,
    };
    for item in &mut scheme.items {
        item.enforce_marker_constraints();
    }
    scheme
}

pub(crate) fn read_existing_daily_queue_index(path: &Path) -> Result<Vec<DailyQueueIndexEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let env = crate::files::parse_workspace_envelope(&raw, path)?;
    crate::files::validate_workspace_version(env.version)?;
    Ok(env.workspace.daily_queue)
}

/// Decode a scheme file, detecting its format. Files written by this version are
/// XML; the markdown reader is kept only to migrate pre-XML files in place.
fn decode_scheme_any(raw: &str, path: &Path, id: SchemeId) -> Result<SchemeFile> {
    if raw.trim_start().starts_with("<?xml") {
        decode_scheme_xml(raw, path, id)
    } else {
        decode_scheme_file(raw, path, id)
    }
}

pub(crate) fn read_scheme_file(path: &Path, id: SchemeId) -> Result<SchemeFile> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    decode_scheme_any(&raw, path, id)
}

pub(crate) fn read_daily_queue_file(
    base_dir: &Path,
    date: NaiveDate,
    id: SchemeId,
) -> Result<SchemeFile> {
    let path = scheme_file_path(base_dir, id);
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    decode_scheme_any(&raw, &path, id)
        .with_context(|| format!("read daily queue scheme for {date}"))
}

pub(crate) fn write_scheme_file(
    base_dir: &Path,
    workspace: &Workspace,
    scheme: &Scheme,
) -> Result<()> {
    let path = scheme_path_for_workspace(base_dir, workspace, scheme.id)?.ok_or_else(|| {
        anyhow!(
            "scheme {} is not in the workspace tree or archive",
            scheme.id
        )
    })?;
    let xml = encode_scheme_xml(scheme)?;
    write_atomic(&path, xml.as_bytes())
}

pub fn scheme_path_for_workspace(
    base_dir: &Path,
    workspace: &Workspace,
    scheme_id: SchemeId,
) -> Result<Option<PathBuf>> {
    if !workspace.schemes.contains_key(&scheme_id) {
        return Ok(None);
    }
    Ok(Some(scheme_file_path(base_dir, scheme_id)))
}

pub(crate) fn scheme_path_for_index(
    base_dir: &Path,
    _root: FolderId,
    _folders: &HashMap<FolderId, Folder>,
    _recently_deleted: &[SchemeId],
    scheme_id: SchemeId,
    _scheme_name: &str,
) -> Result<PathBuf> {
    Ok(scheme_file_path(base_dir, scheme_id))
}

pub(crate) fn ensure_scheme_directories(base_dir: &Path, workspace: &Workspace) -> Result<()> {
    let _ = workspace;
    fs::create_dir_all(schemes_dir(base_dir))
        .with_context(|| format!("create {}", schemes_dir(base_dir).display()))?;
    Ok(())
}

pub(crate) fn prune_removed_scheme_files(base_dir: &Path, workspace: &Workspace) -> Result<()> {
    let retained_files = retained_scheme_paths(base_dir, workspace)?;
    let retained_dirs = HashSet::from([schemes_dir(base_dir)]);
    let root = schemes_dir(base_dir);
    if !root.exists() {
        return Ok(());
    }
    prune_scheme_dir(&root, &retained_files, &retained_dirs)?;
    Ok(())
}

pub(crate) fn write_daily_backup(base_dir: &Path, workspace_json: &str, workspace: &Workspace) {
    let backup_dir = base_dir.join("backups").join(weekday_name());
    let _ = fs::create_dir_all(schemes_dir(&backup_dir));
    let _ = fs::write(backup_dir.join("workspace.json"), workspace_json);
    for scheme in workspace.schemes.values() {
        let Ok(Some(path)) = scheme_path_for_workspace(&backup_dir, workspace, scheme.id) else {
            continue;
        };
        if let Ok(xml) = encode_scheme_xml(scheme) {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, xml);
        }
    }
}

fn retained_scheme_paths(base_dir: &Path, workspace: &Workspace) -> Result<HashSet<PathBuf>> {
    let mut retained = HashSet::new();
    let mut retained_keys = HashSet::new();
    let mut ids = HashSet::new();
    for id in workspace
        .schemes
        .keys()
        .chain(workspace.daily_queue.values())
    {
        if !ids.insert(*id) {
            continue;
        }
        let path = scheme_file_path(base_dir, *id);
        if !retained_keys.insert(path_key(&path)) {
            return Err(anyhow!(
                "multiple schemes resolve to the same file {}",
                path.display()
            ));
        }
        retained.insert(path);
    }
    Ok(retained)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_ascii_lowercase()
}

fn scheme_file_path(base_dir: &Path, id: SchemeId) -> PathBuf {
    schemes_dir(base_dir).join(format!("{id}.{SCHEME_EXT}"))
}

fn prune_scheme_dir(
    dir: &Path,
    retained_files: &HashSet<PathBuf>,
    retained_dirs: &HashSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            prune_scheme_dir(&path, retained_files, retained_dirs)?;
            if !retained_dirs.contains(&path) && is_empty_dir(&path)? {
                fs::remove_dir(&path).with_context(|| format!("remove {}", path.display()))?;
            }
            continue;
        }
        let ext = path.extension().and_then(|ext| ext.to_str());
        if matches!(ext, Some("knotq" | "json")) && !retained_files.contains(&path) {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    Ok(())
}

fn is_empty_dir(path: &Path) -> Result<bool> {
    Ok(fs::read_dir(path)
        .with_context(|| format!("read {}", path.display()))?
        .next()
        .is_none())
}

fn weekday_name() -> &'static str {
    match chrono::Local::now().weekday() {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}
