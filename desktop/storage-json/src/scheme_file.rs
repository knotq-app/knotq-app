use anyhow::{anyhow, Context, Result};
use chrono::{Datelike, NaiveDate};
use knotq_model::{Item, Scheme, SchemeId, Workspace};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use crate::{
    files::{write_atomic, SCHEME_SCHEMA_VERSION},
    paths::{daily_queue_file_path, legacy_daily_queue_file_path, scheme_file_path},
    schema::{DailyQueueIndexEntry, SchemeIndex, WorkspaceEnvelope},
};

#[derive(Serialize, Deserialize)]
struct SchemeEnvelope {
    version: u32,
    scheme: SchemeFile,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SchemeFile {
    pub(crate) id: SchemeId,
    pub(crate) items: Vec<Item>,
}

pub(crate) fn scheme_from_index(index: SchemeIndex, items: Vec<Item>) -> Scheme {
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

pub(crate) fn read_scheme_file(base_dir: &Path, id: SchemeId) -> Result<SchemeFile> {
    let path = scheme_file_path(base_dir, id);
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let env: SchemeEnvelope =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    validate_scheme_version(env.version, &path, "scheme")?;
    Ok(env.scheme)
}

pub(crate) fn read_daily_queue_file(base_dir: &Path, date: NaiveDate) -> Result<SchemeFile> {
    let mut path = daily_queue_file_path(base_dir, date);
    if !path.exists() {
        let legacy_path = legacy_daily_queue_file_path(base_dir, date);
        if legacy_path.exists() {
            path = legacy_path;
        }
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let env: SchemeEnvelope =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    validate_scheme_version(env.version, &path, "daily queue")?;
    Ok(env.scheme)
}

pub(crate) fn write_scheme_file(base_dir: &Path, scheme: &Scheme) -> Result<()> {
    let json = serde_json::to_string_pretty(&scheme_envelope(scheme))?;
    write_atomic(&scheme_file_path(base_dir, scheme.id), json.as_bytes())
}

pub(crate) fn write_daily_queue_file(
    base_dir: &Path,
    date: NaiveDate,
    scheme: &Scheme,
) -> Result<()> {
    let json = serde_json::to_string_pretty(&scheme_envelope(scheme))?;
    write_atomic(&daily_queue_file_path(base_dir, date), json.as_bytes())
}

pub(crate) fn prune_removed_scheme_files(
    schemes_dir: &Path,
    retained_ids: &HashSet<SchemeId>,
) -> Result<()> {
    for entry in
        fs::read_dir(schemes_dir).with_context(|| format!("read {}", schemes_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(uuid) = Uuid::parse_str(stem) else {
            continue;
        };
        if !retained_ids.contains(&SchemeId(uuid)) {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
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
    let _ = fs::create_dir_all(backup_dir.join("schemes"));
    let _ = fs::write(backup_dir.join("workspace.json"), workspace_json);
    let daily_ids: HashSet<SchemeId> = workspace.daily_queue.values().copied().collect();
    for scheme in workspace
        .schemes
        .values()
        .filter(|scheme| !daily_ids.contains(&scheme.id))
    {
        if let Ok(json) = serde_json::to_string_pretty(&scheme_envelope(scheme)) {
            let _ = fs::write(
                backup_dir
                    .join("schemes")
                    .join(format!("{}.json", scheme.id)),
                json,
            );
        }
    }
    for (date, scheme_id) in &workspace.daily_queue {
        let Some(scheme) = workspace.schemes.get(scheme_id) else {
            continue;
        };
        if let Ok(json) = serde_json::to_string_pretty(&scheme_envelope(scheme)) {
            let path = daily_queue_file_path(&backup_dir, *date);
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, json);
        }
    }
}

fn scheme_envelope(scheme: &Scheme) -> SchemeEnvelope {
    SchemeEnvelope {
        version: SCHEME_SCHEMA_VERSION,
        scheme: SchemeFile {
            id: scheme.id,
            items: scheme.items.clone(),
        },
    }
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
            if day_path.extension().and_then(|ext| ext.to_str()) != Some("knotq") {
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

fn validate_scheme_version(version: u32, path: &Path, label: &str) -> Result<()> {
    if version != SCHEME_SCHEMA_VERSION {
        return Err(anyhow!(
            "unsupported {label} schema version {} in {}, expected {}",
            version,
            path.display(),
            SCHEME_SCHEMA_VERSION
        ));
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
