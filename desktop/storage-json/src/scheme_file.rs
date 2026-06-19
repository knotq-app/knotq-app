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

struct DecodedSchemeFile {
    file: SchemeFile,
    should_rewrite: bool,
}

/// Decode a scheme file, detecting its format. Files written by this version are
/// XML; the markdown reader is kept only to migrate pre-XML files in place.
fn decode_scheme_any(raw: &str, path: &Path, id: SchemeId) -> Result<DecodedSchemeFile> {
    if raw_looks_like_xml_scheme(raw) {
        let file = decode_scheme_xml(raw, path, id)?;
        let (file, recovered_wrapped_xml) = recover_wrapped_xml_text(file, path, id)?;
        return Ok(DecodedSchemeFile {
            file,
            should_rewrite: recovered_wrapped_xml || !raw_has_xml_declaration(raw),
        });
    }

    Ok(DecodedSchemeFile {
        file: decode_scheme_file(raw, path, id)?,
        should_rewrite: false,
    })
}

pub(crate) fn read_scheme_file(path: &Path, id: SchemeId) -> Result<SchemeFile> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let decoded = decode_scheme_any(&raw, path, id)?;
    rewrite_recovered_scheme_file(path, &decoded)?;
    Ok(decoded.file)
}

pub(crate) fn read_daily_queue_file(
    base_dir: &Path,
    date: NaiveDate,
    id: SchemeId,
) -> Result<SchemeFile> {
    let path = scheme_file_path(base_dir, id);
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let decoded = decode_scheme_any(&raw, &path, id)
        .with_context(|| format!("read daily queue scheme for {date}"))?;
    rewrite_recovered_scheme_file(&path, &decoded)
        .with_context(|| format!("rewrite recovered daily queue scheme for {date}"))?;
    Ok(decoded.file)
}

pub(crate) fn repair_scheme_file_format(path: &Path) -> Result<bool> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(false);
    }
    let id = scheme_id_from_path(path);
    let decoded = decode_scheme_any(&raw, path, id)?;
    if decoded.should_rewrite {
        rewrite_recovered_scheme_file(path, &decoded)?;
        return Ok(true);
    }
    Ok(false)
}

fn raw_has_xml_declaration(raw: &str) -> bool {
    raw.trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("<?xml")
}

fn raw_looks_like_xml_scheme(raw: &str) -> bool {
    let trimmed = raw.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("<?xml") {
        return true;
    }
    trimmed.starts_with("<scheme")
}

fn recover_wrapped_xml_text(
    file: SchemeFile,
    path: &Path,
    id: SchemeId,
) -> Result<(SchemeFile, bool)> {
    let Some(raw) = xml_text_from_wrapped_items(&file.items) else {
        return Ok((file, false));
    };
    let recovered = decode_scheme_xml(&raw, path, id).with_context(|| {
        format!(
            "recover scheme XML that was previously loaded as text in {}",
            path.display()
        )
    })?;
    if recovered.items.is_empty() {
        return Ok((file, false));
    }
    Ok((recovered, true))
}

fn xml_text_from_wrapped_items(items: &[knotq_model::Item]) -> Option<String> {
    if items.len() < 3 || !items.iter().all(is_wrapped_xml_text_item) {
        return None;
    }

    let lines: Vec<String> = items
        .iter()
        .map(|item| {
            let mut line = "  ".repeat(item.indent as usize);
            line.push_str(&item.text());
            line
        })
        .collect();
    let non_empty: Vec<&str> = lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    let first = non_empty.first().copied()?;
    let starts_with_scheme = if first.starts_with("<?xml") {
        non_empty
            .iter()
            .skip(1)
            .any(|line| line.starts_with("<scheme"))
    } else {
        first.starts_with("<scheme")
    };
    if !starts_with_scheme || non_empty.last().copied() != Some("</scheme>") {
        return None;
    }
    if !non_empty.iter().any(|line| line.starts_with("<item")) {
        return None;
    }

    Some(lines.join("\n"))
}

fn is_wrapped_xml_text_item(item: &knotq_model::Item) -> bool {
    item.marker == knotq_model::ItemMarker::Blank
        && item.start.is_none()
        && item.end.is_none()
        && item.available.is_none()
        && item.repeats.is_none()
        && item.priority.is_none()
        && item.external.is_none()
        && item.state.len() == 1
        && item.state.first().is_none_or(|state| {
            state.occurrence == knotq_model::OccurrenceId::Single && state.state.is_default()
        })
        && !item.has_images()
        && !item.has_table()
        && item.content.is_text()
}

fn rewrite_recovered_scheme_file(path: &Path, decoded: &DecodedSchemeFile) -> Result<()> {
    if !decoded.should_rewrite {
        return Ok(());
    }

    let scheme = Scheme {
        id: decoded.file.id,
        name: String::new(),
        color_index: 0,
        gsync: false,
        source: Default::default(),
        items: decoded.file.items.clone(),
    };
    let xml = encode_scheme_xml(&scheme)?;
    write_atomic(path, xml.as_bytes()).with_context(|| format!("rewrite {}", path.display()))
}

fn scheme_id_from_path(path: &Path) -> SchemeId {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.parse::<SchemeId>().ok())
        .unwrap_or_else(SchemeId::new)
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

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::{Item, ItemMarker};

    #[test]
    fn decodes_xml_scheme_without_declaration() {
        let id = SchemeId::new();
        let mut scheme = Scheme::new("Recovered", 0);
        scheme.id = id;
        scheme
            .items
            .push(Item::new("file taxes").with_marker(ItemMarker::Checkbox));

        let raw = without_xml_declaration(&encode_scheme_xml(&scheme).unwrap());
        let decoded = decode_scheme_any(&raw, Path::new("scheme.knotq"), id).unwrap();

        assert!(decoded.should_rewrite);
        assert_eq!(decoded.file.items.len(), 1);
        assert_eq!(decoded.file.items[0].text(), "file taxes");
        assert_eq!(decoded.file.items[0].marker, ItemMarker::Checkbox);
    }

    #[test]
    fn recovers_scheme_xml_that_was_saved_as_literal_text_items() {
        let id = SchemeId::new();
        let mut scheme = Scheme::new("Recovered", 0);
        scheme.id = id;
        scheme.items.push(Item::new("plan trip"));
        scheme
            .items
            .push(Item::new("book flights").with_marker(ItemMarker::Checkbox));

        let original_xml = without_xml_declaration(&encode_scheme_xml(&scheme).unwrap());
        let text_items = decode_scheme_file(&original_xml, Path::new("corrupt.knotq"), id)
            .unwrap()
            .items;
        let corrupt_scheme = Scheme {
            id,
            name: String::new(),
            color_index: 0,
            gsync: false,
            source: Default::default(),
            items: text_items,
        };
        let wrapped_xml = encode_scheme_xml(&corrupt_scheme).unwrap();

        let decoded = decode_scheme_any(&wrapped_xml, Path::new("wrapped.knotq"), id).unwrap();

        assert!(decoded.should_rewrite);
        assert_eq!(decoded.file.items.len(), 2);
        assert_eq!(decoded.file.items[0].text(), "plan trip");
        assert_eq!(decoded.file.items[1].text(), "book flights");
        assert_eq!(decoded.file.items[1].marker, ItemMarker::Checkbox);
    }

    fn without_xml_declaration(xml: &str) -> String {
        xml.lines().skip(1).collect::<Vec<_>>().join("\n")
    }
}
