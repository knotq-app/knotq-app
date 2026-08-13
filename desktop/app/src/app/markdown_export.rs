use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use gpui::{Context, PathPromptOptions, Window};
use knotq_model::{FolderId, Item, ItemContent, ItemMarker, NodeRef, Scheme, SchemeId, Workspace};
use knotq_storage_json::image_asset_path;

use super::KnotQApp;

impl KnotQApp {
    pub(crate) fn export_workspace_as_markdown(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a folder for the Markdown export".into()),
        });
        let workspace = self.workspace.clone();
        cx.spawn(async move |_app, _cx| {
            let Some(destination) = paths.await.ok().and_then(|result| result.ok()).flatten().and_then(|mut paths| paths.pop()) else {
                return;
            };
            // The chosen directory is never modified directly: every export gets
            // a fresh, collision-safe folder so existing Markdown is protected.
            std::thread::spawn(move || {
                if let Err(error) = export_workspace(&workspace, &destination) {
                    eprintln!("KnotQ Markdown export failed: {error:#}");
                }
            });
        })
        .detach();
    }
}

fn export_workspace(workspace: &Workspace, destination: &Path) -> Result<PathBuf> {
    fs::create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    let root = unique_path(destination, "KnotQ Markdown Export", None);
    fs::create_dir(&root).with_context(|| format!("create {}", root.display()))?;

    let mut exported_schemes = HashSet::new();
    let mut visited_folders = HashSet::new();
    export_folder(workspace, workspace.root, &root, &mut exported_schemes, &mut visited_folders)?;

    // Daily Queue documents are intentionally indexed by date rather than
    // necessarily represented in the ordinary folder tree.
    if !workspace.daily_queue.is_empty() {
        let daily = unique_path(&root, "Daily Queue", None);
        fs::create_dir(&daily)?;
        for (date, scheme_id) in &workspace.daily_queue {
            if let Some(scheme) = workspace.schemes.get(scheme_id) {
                let path = unique_path(&daily, &format!("{date} {}", scheme.name), Some("md"));
                write_scheme_markdown(scheme, &path)?;
                exported_schemes.insert(*scheme_id);
            }
        }
    }

    // Preserve active documents even if a malformed/legacy workspace tree does
    // not currently link them from a folder.
    let remaining: Vec<_> = workspace
        .schemes
        .iter()
        .filter(|(id, _)| !exported_schemes.contains(id) && !workspace.recently_deleted.contains(id))
        .collect();
    if !remaining.is_empty() {
        let unsorted = unique_path(&root, "Unsorted", None);
        fs::create_dir(&unsorted)?;
        for (id, scheme) in remaining {
            let path = unique_path(&unsorted, &scheme.name, Some("md"));
            write_scheme_markdown(scheme, &path)?;
            exported_schemes.insert(*id);
        }
    }
    Ok(root)
}

fn export_folder(
    workspace: &Workspace,
    folder_id: FolderId,
    destination: &Path,
    exported_schemes: &mut HashSet<SchemeId>,
    visited_folders: &mut HashSet<FolderId>,
) -> Result<()> {
    if !visited_folders.insert(folder_id) {
        return Ok(());
    }
    let Some(folder) = workspace.folders.get(&folder_id) else {
        return Ok(());
    };
    for node in &folder.children {
        match *node {
            NodeRef::Folder(id) => {
                let Some(child) = workspace.folders.get(&id) else { continue };
                let path = unique_path(destination, &child.name, None);
                fs::create_dir(&path)?;
                export_folder(workspace, id, &path, exported_schemes, visited_folders)?;
            }
            NodeRef::Scheme(id) => {
                let Some(scheme) = workspace.schemes.get(&id) else { continue };
                if workspace.recently_deleted.contains(&id) || workspace.is_daily_queue_scheme(id) {
                    continue;
                }
                let path = unique_path(destination, &scheme.name, Some("md"));
                write_scheme_markdown(scheme, &path)?;
                exported_schemes.insert(id);
            }
        }
    }
    Ok(())
}

fn write_scheme_markdown(scheme: &Scheme, path: &Path) -> Result<()> {
    let mut markdown = format!("# {}\n\n", scheme.name.trim());
    for item in &scheme.items {
        markdown.push_str(&markdown_item(item, path.parent().expect("export file has a parent"))?);
    }
    fs::write(path, markdown).with_context(|| format!("write {}", path.display()))
}

fn markdown_item(item: &Item, directory: &Path) -> Result<String> {
    let indent = "  ".repeat(item.indent as usize);
    let prefix = match item.marker {
        ItemMarker::Blank => "",
        ItemMarker::Bullet => "- ",
        ItemMarker::Numbered => "1. ",
        ItemMarker::Checkbox if item.single_state().is_done() => "- [x] ",
        ItemMarker::Checkbox => "- [ ] ",
    };
    let content = match &item.content {
        ItemContent::Text { text } => text.clone(),
        ItemContent::Image(image) => {
            let extension = image.format.extension();
            let filename = format!("{}.{}", image.asset, extension);
            let source = image_asset_path(image.asset, extension);
            let assets = directory.join("assets");
            fs::create_dir_all(&assets)?;
            if source.exists() {
                fs::copy(&source, assets.join(&filename))
                    .with_context(|| format!("copy image {}", source.display()))?;
            }
            format!("![image](assets/{filename})")
        }
        ItemContent::Table(table) => {
            let header = table.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>();
            let divider = std::iter::repeat("---").take(header.len()).collect::<Vec<_>>();
            let mut table_markdown = format!("| {} |\n| {} |", header.join(" | "), divider.join(" | "));
            for row in &table.rows {
                let cells = row.cells.iter().map(|cell| cell.summary_text()).collect::<Vec<_>>();
                table_markdown.push_str(&format!("\n| {} |", cells.join(" | ")));
            }
            table_markdown
        }
    };
    let mut line = format!("{indent}{prefix}{content}");
    if item.start.is_some() || item.end.is_some() || item.available.is_some() || item.repeats.is_some() {
        let mut metadata = Vec::new();
        if let Some(start) = item.start { metadata.push(format!("start: {}", start.to_rfc3339())); }
        if let Some(end) = item.end { metadata.push(format!("end: {}", end.to_rfc3339())); }
        if let Some(available) = item.available { metadata.push(format!("available: {}", available.to_rfc3339())); }
        if let Some(repeats) = &item.repeats { metadata.push(format!("repeat: {repeats:?}")); }
        line.push_str(&format!("  <!-- {} -->", metadata.join("; ")));
    }
    line.push_str("\n\n");
    Ok(line)
}

fn unique_path(directory: &Path, name: &str, extension: Option<&str>) -> PathBuf {
    let stem = sanitized_name(name);
    let mut index = 1;
    loop {
        let suffix = (index > 1).then(|| format!(" {index}")).unwrap_or_default();
        let filename = match extension {
            Some(extension) => format!("{stem}{suffix}.{extension}"),
            None => format!("{stem}{suffix}"),
        };
        let candidate = directory.join(filename);
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}

fn sanitized_name(name: &str) -> String {
    let name = name.trim();
    let cleaned: String = name
        .chars()
        .map(|character| if matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || character.is_control() { ' ' } else { character })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() { "Untitled".into() } else { cleaned.chars().take(120).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_names_are_safe_and_collision_free() {
        let temporary = std::env::temp_dir().join(format!("knotq-export-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temporary).unwrap();
        let first = unique_path(&temporary, "A/B", Some("md"));
        fs::write(&first, "first").unwrap();
        let second = unique_path(&temporary, "A/B", Some("md"));
        assert_eq!(first.file_name().unwrap(), "A B.md");
        assert_eq!(second.file_name().unwrap(), "A B 2.md");
        fs::remove_dir_all(temporary).unwrap();
    }
}
