use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, NaiveDate};
use knotq_model::{
    validate_workspace_node_name, Folder, FolderId, NodeRef, Scheme, SchemeId, Workspace,
    WorkspaceNodeNameKind,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    files::write_atomic,
    paths::{daily_queue_file_path, schemes_dir},
    schema::{DailyQueueIndexEntry, SchemeIndex, WorkspaceEnvelope},
    scheme_markdown::{decode_scheme_file, encode_scheme_file},
};

const SCHEME_EXT: &str = "knotq";
const TRASH_DIR: &str = ".trash";

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
    let env: WorkspaceEnvelope = serde_json::from_str(&raw).context("parse workspace index")?;
    crate::files::validate_workspace_version(env.version)?;
    Ok(env.workspace.daily_queue)
}

pub(crate) fn read_scheme_file(path: &Path, id: SchemeId) -> Result<SchemeFile> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    decode_scheme_file(&raw, path, id)
}

pub(crate) fn read_daily_queue_file(
    base_dir: &Path,
    date: NaiveDate,
    id: SchemeId,
) -> Result<SchemeFile> {
    let path = daily_queue_file_path(base_dir, date);
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    decode_scheme_file(&raw, &path, id)
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
    let markdown = encode_scheme_file(scheme)?;
    write_atomic(&path, markdown.as_bytes())
}

pub(crate) fn write_daily_queue_file(
    base_dir: &Path,
    date: NaiveDate,
    scheme: &Scheme,
) -> Result<()> {
    let markdown = encode_scheme_file(scheme)?;
    write_atomic(&daily_queue_file_path(base_dir, date), markdown.as_bytes())
}

pub fn scheme_path_for_workspace(
    base_dir: &Path,
    workspace: &Workspace,
    scheme_id: SchemeId,
) -> Result<Option<PathBuf>> {
    let Some(scheme) = workspace.schemes.get(&scheme_id) else {
        return Ok(None);
    };
    if workspace.is_scheme_deleted(scheme_id) {
        return deleted_scheme_path(base_dir, &scheme.name, scheme_id).map(Some);
    }
    let Some(parent) = active_scheme_parent(&workspace.folders, scheme_id) else {
        return Ok(None);
    };
    active_scheme_path(
        base_dir,
        workspace.root,
        &workspace.folders,
        parent,
        &scheme.name,
    )
    .map(Some)
}

pub(crate) fn scheme_path_for_index(
    base_dir: &Path,
    root: FolderId,
    folders: &HashMap<FolderId, Folder>,
    recently_deleted: &[SchemeId],
    scheme_id: SchemeId,
    scheme_name: &str,
) -> Result<PathBuf> {
    if recently_deleted.contains(&scheme_id) {
        return deleted_scheme_path(base_dir, scheme_name, scheme_id);
    }
    let parent = active_scheme_parent(folders, scheme_id)
        .ok_or_else(|| anyhow!("scheme {scheme_id} is not referenced by any folder"))?;
    active_scheme_path(base_dir, root, folders, parent, scheme_name)
}

pub(crate) fn ensure_scheme_directories(base_dir: &Path, workspace: &Workspace) -> Result<()> {
    retained_scheme_paths(base_dir, workspace)?;
    let dirs = retained_scheme_dirs(base_dir, workspace)?;
    for dir in dirs {
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    }
    Ok(())
}

pub(crate) fn prune_removed_scheme_files(base_dir: &Path, workspace: &Workspace) -> Result<()> {
    let retained_files = retained_scheme_paths(base_dir, workspace)?;
    let retained_dirs = retained_scheme_dirs(base_dir, workspace)?;
    let root = schemes_dir(base_dir);
    if !root.exists() {
        return Ok(());
    }
    prune_scheme_dir(&root, &retained_files, &retained_dirs)?;
    Ok(())
}

pub(crate) fn prune_removed_daily_queue_files(
    daily_queue_dir: &Path,
    workspace: &Workspace,
) -> Result<()> {
    if !daily_queue_dir.exists() {
        return Ok(());
    }
    let retained: HashSet<PathBuf> = workspace
        .daily_queue
        .keys()
        .map(|date| {
            daily_queue_file_path(
                daily_queue_dir.parent().unwrap_or_else(|| Path::new(".")),
                *date,
            )
        })
        .collect();
    for year_entry in fs::read_dir(daily_queue_dir)
        .with_context(|| format!("read {}", daily_queue_dir.display()))?
    {
        let year_path = year_entry?.path();
        if !year_path.is_dir() {
            continue;
        }
        prune_daily_queue_months(&year_path, &retained)?;
    }
    Ok(())
}

pub(crate) fn write_daily_backup(base_dir: &Path, workspace_json: &str, workspace: &Workspace) {
    let backup_dir = base_dir.join("backups").join(weekday_name());
    let _ = fs::create_dir_all(schemes_dir(&backup_dir));
    let _ = fs::write(backup_dir.join("workspace.json"), workspace_json);
    let daily_ids: HashSet<SchemeId> = workspace.daily_queue.values().copied().collect();
    for scheme in workspace
        .schemes
        .values()
        .filter(|scheme| !daily_ids.contains(&scheme.id))
    {
        let Ok(Some(path)) = scheme_path_for_workspace(&backup_dir, workspace, scheme.id) else {
            continue;
        };
        if let Ok(markdown) = encode_scheme_file(scheme) {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, markdown);
        }
    }
    for (date, scheme_id) in &workspace.daily_queue {
        let Some(scheme) = workspace.schemes.get(scheme_id) else {
            continue;
        };
        if let Ok(markdown) = encode_scheme_file(scheme) {
            let path = daily_queue_file_path(&backup_dir, *date);
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, markdown);
        }
    }
}

fn retained_scheme_paths(base_dir: &Path, workspace: &Workspace) -> Result<HashSet<PathBuf>> {
    let daily_ids: HashSet<SchemeId> = workspace.daily_queue.values().copied().collect();
    let mut retained = HashSet::new();
    let mut retained_keys = HashSet::new();
    for id in workspace
        .schemes
        .keys()
        .filter(|id| !daily_ids.contains(id))
    {
        let path = scheme_path_for_workspace(base_dir, workspace, *id)?.ok_or_else(|| {
            anyhow!("scheme {id} is not in the workspace tree or recently deleted list")
        })?;
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

fn retained_scheme_dirs(base_dir: &Path, workspace: &Workspace) -> Result<HashSet<PathBuf>> {
    let root = schemes_dir(base_dir);
    let mut dirs = HashSet::from([root.clone()]);
    let mut dir_keys = HashSet::from([path_key(&root)]);
    for (id, folder) in &workspace.folders {
        if *id == workspace.root {
            continue;
        }
        validate_folder_name(&folder.name)?;
        let dir = active_folder_path(base_dir, workspace.root, &workspace.folders, *id)?;
        if !dir_keys.insert(path_key(&dir)) {
            return Err(anyhow!(
                "multiple folders resolve to the same directory {}",
                dir.display()
            ));
        }
        dirs.insert(dir);
    }
    if !workspace.recently_deleted.is_empty() {
        dirs.insert(root.join(TRASH_DIR));
    }
    Ok(dirs)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_ascii_lowercase()
}

fn active_scheme_parent(
    folders: &HashMap<FolderId, Folder>,
    scheme_id: SchemeId,
) -> Option<FolderId> {
    folders.iter().find_map(|(folder_id, folder)| {
        folder
            .children
            .iter()
            .any(|child| *child == NodeRef::Scheme(scheme_id))
            .then_some(*folder_id)
    })
}

fn active_scheme_path(
    base_dir: &Path,
    root: FolderId,
    folders: &HashMap<FolderId, Folder>,
    parent: FolderId,
    scheme_name: &str,
) -> Result<PathBuf> {
    validate_scheme_name(scheme_name)?;
    let file_name = scheme_file_name(scheme_name)?;
    Ok(active_folder_path(base_dir, root, folders, parent)?.join(file_name))
}

fn active_folder_path(
    base_dir: &Path,
    root: FolderId,
    folders: &HashMap<FolderId, Folder>,
    folder_id: FolderId,
) -> Result<PathBuf> {
    let mut segments = Vec::new();
    let mut seen = HashSet::new();
    let mut current = folder_id;
    while current != root {
        if !seen.insert(current) {
            return Err(anyhow!("folder tree contains a cycle at folder {current}"));
        }
        let folder = folders
            .get(&current)
            .ok_or_else(|| anyhow!("folder {current} missing for scheme path"))?;
        validate_folder_name(&folder.name)?;
        segments.push(folder.name.clone());
        current = folder
            .parent
            .ok_or_else(|| anyhow!("folder {current} is missing a parent"))?;
    }
    let mut path = schemes_dir(base_dir);
    for segment in segments.iter().rev() {
        path.push(segment);
    }
    Ok(path)
}

fn deleted_scheme_path(base_dir: &Path, scheme_name: &str, scheme_id: SchemeId) -> Result<PathBuf> {
    validate_scheme_name(scheme_name)?;
    Ok(schemes_dir(base_dir).join(TRASH_DIR).join(format!(
        "{scheme_name} ({}).{SCHEME_EXT}",
        short_id(scheme_id)
    )))
}

fn scheme_file_name(scheme_name: &str) -> Result<String> {
    validate_scheme_name(scheme_name)?;
    Ok(format!("{scheme_name}.{SCHEME_EXT}"))
}

fn validate_folder_name(name: &str) -> Result<()> {
    validate_workspace_node_name(name, WorkspaceNodeNameKind::Folder)
        .with_context(|| format!("invalid folder name {name:?}"))
}

fn validate_scheme_name(name: &str) -> Result<()> {
    validate_workspace_node_name(name, WorkspaceNodeNameKind::Scheme)
        .with_context(|| format!("invalid scheme name {name:?}"))
}

fn short_id(id: SchemeId) -> String {
    id.to_string().chars().take(8).collect()
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

fn prune_daily_queue_months(year_path: &Path, retained: &HashSet<PathBuf>) -> Result<()> {
    for month_entry in
        fs::read_dir(year_path).with_context(|| format!("read {}", year_path.display()))?
    {
        let month_path = month_entry?.path();
        if !month_path.is_dir() {
            continue;
        }
        for day_entry in
            fs::read_dir(&month_path).with_context(|| format!("read {}", month_path.display()))?
        {
            let day_path = day_entry?.path();
            if day_path.extension().and_then(|ext| ext.to_str()) != Some(SCHEME_EXT) {
                continue;
            }
            if !retained.contains(&day_path) {
                fs::remove_file(&day_path)
                    .with_context(|| format!("remove {}", day_path.display()))?;
            }
        }
    }
    Ok(())
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
