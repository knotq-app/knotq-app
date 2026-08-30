use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Local, Utc};
use knotq_date_util::{format_contextual_datetime, format_time};
use knotq_model::{
    FolderId, ImageInline, Item, ItemContent, ItemMarker, NodeRef, Recurrence, Scheme, Table,
    TableCell, TimeFormat, Workspace,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Export every folder/scheme reachable from the workspace root into a fresh,
/// timestamped directory under `destination`, one Markdown file per scheme,
/// mirroring the sidebar's folder structure. Returns the directory created.
///
/// Archived/deleted folders and schemes are not reachable from `root` (they're
/// deliberately detached, see [`Workspace::recently_deleted_folders`]) so they
/// are excluded automatically, matching what the sidebar shows. The daily
/// queue is excluded too — it isn't part of the folder tree.
pub fn export_workspace_to_markdown(
    workspace: &Workspace,
    destination: &Path,
    time_format: TimeFormat,
) -> Result<PathBuf> {
    let stamp = Local::now().format("%Y-%m-%d %H%M%S");
    let root_dir = unique_path(destination, &format!("KnotQ Export {stamp}"));
    fs::create_dir_all(&root_dir)
        .with_context(|| format!("create export directory {}", root_dir.display()))?;

    let assets_dir = root_dir.join("assets");
    let mut copied_assets = HashSet::new();
    export_folder(
        workspace,
        workspace.root,
        &root_dir,
        0,
        time_format,
        &assets_dir,
        &mut copied_assets,
    )?;

    Ok(root_dir)
}

fn export_folder(
    workspace: &Workspace,
    folder_id: FolderId,
    dir: &Path,
    depth: usize,
    time_format: TimeFormat,
    assets_dir: &Path,
    copied_assets: &mut HashSet<Uuid>,
) -> Result<()> {
    let Some(folder) = workspace.folders.get(&folder_id) else {
        return Ok(());
    };
    let mut used_names: HashSet<String> = HashSet::new();
    for child in &folder.children {
        match child {
            NodeRef::Folder(id) => {
                let Some(sub) = workspace.folders.get(id) else {
                    continue;
                };
                let name = unique_name(&mut used_names, &sanitize_name(&sub.name, "Folder"));
                let sub_dir = dir.join(&name);
                fs::create_dir_all(&sub_dir)
                    .with_context(|| format!("create folder {}", sub_dir.display()))?;
                export_folder(
                    workspace,
                    *id,
                    &sub_dir,
                    depth + 1,
                    time_format,
                    assets_dir,
                    copied_assets,
                )?;
            }
            NodeRef::Scheme(id) => {
                let Some(scheme) = workspace.schemes.get(id) else {
                    continue;
                };
                let name = unique_name(&mut used_names, &sanitize_name(&scheme.name, "Untitled"));
                let file_path = dir.join(format!("{name}.md"));
                let markdown =
                    render_scheme_markdown(scheme, time_format, depth, assets_dir, copied_assets)?;
                fs::write(&file_path, markdown)
                    .with_context(|| format!("write {}", file_path.display()))?;
            }
        }
    }
    Ok(())
}

fn render_scheme_markdown(
    scheme: &Scheme,
    time_format: TimeFormat,
    depth: usize,
    assets_dir: &Path,
    copied_assets: &mut HashSet<Uuid>,
) -> Result<String> {
    let mut out = String::new();
    for item in &scheme.items {
        render_item_block(
            item,
            time_format,
            depth,
            assets_dir,
            copied_assets,
            &mut out,
        )?;
    }
    Ok(out)
}

fn render_item_block(
    item: &Item,
    time_format: TimeFormat,
    depth: usize,
    assets_dir: &Path,
    copied_assets: &mut HashSet<Uuid>,
    out: &mut String,
) -> Result<()> {
    let indent = "  ".repeat(item.indent as usize);
    match &item.content {
        ItemContent::Table(table) => {
            out.push_str(&render_table_markdown(table));
            out.push('\n');
        }
        ItemContent::Image(image) => {
            let link = export_image_asset(image, depth, assets_dir, copied_assets)?;
            out.push_str(&indent);
            out.push_str(&link);
            out.push('\n');
        }
        ItemContent::Text { .. } => {
            let (prefix, prefix_width) = marker_prefix(item);
            out.push_str(&indent);
            out.push_str(prefix);
            out.push_str(&fix_italic_delimiters(&item.text()));
            out.push('\n');
            if let Some(annotation) = item_annotation(item, time_format) {
                out.push_str(&indent);
                out.push_str(&" ".repeat(prefix_width));
                out.push_str(&annotation);
                out.push('\n');
            }
        }
    }
    Ok(())
}

/// The line's leading markdown syntax, and how many columns wide it is (so an
/// annotation line below it can indent to align under the text).
fn marker_prefix(item: &Item) -> (&'static str, usize) {
    match item.marker {
        ItemMarker::Blank => ("", 0),
        ItemMarker::Bullet => ("- ", 2),
        ItemMarker::Numbered => ("1. ", 3),
        ItemMarker::Checkbox if item.single_state().is_done() => ("- [x] ", 6),
        ItemMarker::Checkbox => ("- [ ] ", 6),
    }
}

/// KnotQ's editor uses `__x__` for italic, but standard GitHub-flavored
/// markdown treats `__` the same as `**` (bold) — rewrite to the
/// single-underscore form so exported files render as italic in ordinary
/// markdown viewers instead of doubling up as bold.
fn fix_italic_delimiters(text: &str) -> String {
    text.replace("__", "_")
}

/// A human-readable "due …" / "at … → …" / "repeats …" line, reusing the same
/// wording (and l10n keys) the editor shows under a checkbox item — but only
/// the common cases: this is a best-effort export, not a full RRULE
/// formatter, so an exotic recurrence (multiple rules, rdates, exceptions)
/// just says "Repeats".
fn item_annotation(item: &Item, time_format: TimeFormat) -> Option<String> {
    if item.marker != ItemMarker::Checkbox {
        return None;
    }
    let mut text = match (item.start, item.end) {
        (Some(start), Some(end)) => format!(
            "{} \u{2192} {}",
            format_when(start, None, time_format),
            format_when(end, Some(start), time_format)
        ),
        (Some(start), None) => knotq_l10n::t_with(
            "editor.annotation.at",
            &[("date", &format_when(start, None, time_format))],
        ),
        (None, Some(end)) => knotq_l10n::t_with(
            "editor.annotation.due",
            &[("date", &format_when(end, None, time_format))],
        ),
        (None, None) => String::new(),
    };
    if let Some(repeat) = &item.repeats {
        if !text.is_empty() {
            text.push_str(" · ");
        }
        text.push_str(&simple_repeat_description(repeat));
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn format_when(
    dt: DateTime<Utc>,
    previous: Option<DateTime<Utc>>,
    time_format: TimeFormat,
) -> String {
    let local = dt.with_timezone(&Local);
    if previous
        .map(|previous| previous.with_timezone(&Local).date_naive() == local.date_naive())
        .unwrap_or(false)
    {
        return format_time(time_format, local);
    }
    let today = Local::now().date_naive();
    if local.date_naive() == today {
        format_time(time_format, local)
    } else {
        format_contextual_datetime(time_format, local, today.year())
    }
}

fn simple_repeat_description(repeat: &Recurrence) -> String {
    let complex = || knotq_l10n::t("editor.repeat.complex").to_string();
    if repeat.rrules.len() != 1 || !repeat.rdates.is_empty() {
        return complex();
    }
    let fields: std::collections::HashMap<String, String> = repeat.rrules[0]
        .trim()
        .trim_start_matches("RRULE:")
        .split(';')
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((
                key.trim().to_ascii_uppercase(),
                value.trim().to_ascii_uppercase(),
            ))
        })
        .collect();
    let interval: usize = fields
        .get("INTERVAL")
        .and_then(|value| value.parse().ok())
        .unwrap_or(1)
        .max(1);
    let one_or_every = |one: &str, every: &str| {
        if interval <= 1 {
            knotq_l10n::t(one).to_string()
        } else {
            knotq_l10n::t_with(every, &[("count", &interval.to_string())])
        }
    };
    match fields.get("FREQ").map(String::as_str) {
        Some("DAILY") => one_or_every("editor.repeat.daily", "editor.repeat.every_days"),
        Some("WEEKLY") => one_or_every("editor.repeat.weekly", "editor.repeat.every_weeks"),
        Some("MONTHLY") => one_or_every("editor.repeat.monthly", "editor.repeat.every_months"),
        Some("YEARLY") => one_or_every("editor.repeat.yearly", "editor.repeat.every_years"),
        _ => complex(),
    }
}

fn render_table_markdown(table: &Table) -> String {
    let mut out = String::new();
    let header: Vec<String> = table
        .columns
        .iter()
        .map(|column| escape_table_cell(&column.name))
        .collect();
    out.push_str(&format!("| {} |\n", header.join(" | ")));
    let divider: Vec<&str> = table.columns.iter().map(|_| "---").collect();
    out.push_str(&format!("| {} |\n", divider.join(" | ")));
    for row in &table.rows {
        let cells: Vec<String> = row.cells.iter().map(render_cell_markdown).collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out
}

/// One GFM table cell: the cell's items (a small sub-document) rendered as
/// marker-prefixed lines joined with `<br>`, since a table cell can't hold a
/// real markdown list.
fn render_cell_markdown(cell: &TableCell) -> String {
    cell.items
        .iter()
        .map(|item| {
            let (prefix, _) = marker_prefix(item);
            escape_table_cell(&format!("{prefix}{}", fix_italic_delimiters(&item.text())))
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn escape_table_cell(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', " ")
}

/// Copy an image asset into the export's shared `assets/` directory (once per
/// asset) and return the markdown image link, relative to wherever the
/// scheme's own file ends up in the mirrored folder tree.
fn export_image_asset(
    image: &ImageInline,
    depth: usize,
    assets_dir: &Path,
    copied_assets: &mut HashSet<Uuid>,
) -> Result<String> {
    let ext = image.format.extension();
    let file_name = format!("{}.{ext}", image.asset);
    if copied_assets.insert(image.asset) {
        let source = crate::paths::image_asset_path(image.asset, ext);
        if source.is_file() {
            fs::create_dir_all(assets_dir)
                .with_context(|| format!("create {}", assets_dir.display()))?;
            fs::copy(&source, assets_dir.join(&file_name))
                .with_context(|| format!("copy image asset {}", source.display()))?;
        }
    }
    let up_to_root = "../".repeat(depth);
    Ok(format!("![]({up_to_root}assets/{file_name})"))
}

/// Sanitize a folder/scheme name into a safe path component: filesystem-unsafe
/// characters become `_`, and an empty result falls back to `fallback`.
fn sanitize_name(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Make `name` unique among names already used in the same directory
/// (case-insensitive, since macOS/Windows filesystems are by default),
/// appending " (2)", " (3)", … as needed, and record whichever name wins.
fn unique_name(used: &mut HashSet<String>, name: &str) -> String {
    if used.insert(name.to_lowercase()) {
        return name.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{name} ({n})");
        if used.insert(candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

/// Make `parent.join(name)` unique against what's actually on disk, the same
/// way (" (2)", " (3)", …) — used only for the top-level export directory,
/// which isn't tracked in any in-memory `used` set.
fn unique_path(parent: &Path, name: &str) -> PathBuf {
    let mut candidate = parent.join(name);
    let mut n = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{name} ({n})"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::{Folder, ItemState, OccurrenceId, OccurrenceState};

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "knotq-export-markdown-test-{}-{label}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn add_scheme(workspace: &mut Workspace, folder: FolderId, name: &str, text: &str) {
        let mut scheme = Scheme::new(name, 0);
        scheme.items.push(Item::new(text));
        let id = scheme.id;
        workspace.schemes.insert(id, scheme);
        workspace
            .folders
            .get_mut(&folder)
            .unwrap()
            .children
            .push(NodeRef::Scheme(id));
    }

    fn entry_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn duplicate_scheme_names_get_deduplicated_suffixes() {
        let mut workspace = Workspace::new();
        let root = workspace.root;
        add_scheme(&mut workspace, root, "Notes", "one");
        add_scheme(&mut workspace, root, "Notes", "two");
        // Sanitizing a name shouldn't collapse two DIFFERENT names into a
        // false collision, but a case-insensitive one still counts.
        add_scheme(&mut workspace, root, "notes", "three");

        let dest = scratch_dir("dup-names");
        let created =
            export_workspace_to_markdown(&workspace, &dest, TimeFormat::TwelveHour).unwrap();

        // Each file keeps its own scheme's actual casing; only the number
        // suffix reflects that "notes" collided case-insensitively with the
        // two "Notes" that came before it.
        assert_eq!(
            entry_names(&created),
            vec!["Notes (2).md", "Notes.md", "notes (3).md"]
        );
    }

    #[test]
    fn nested_folders_are_mirrored_as_real_directories() {
        let mut workspace = Workspace::new();
        let root = workspace.root;
        let child = FolderId::new();
        workspace.folders.insert(
            child,
            Folder {
                id: child,
                name: "Projects".to_string(),
                parent: Some(root),
                children: Vec::new(),
                expanded: true,
            },
        );
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Folder(child));
        add_scheme(&mut workspace, child, "Plan", "hello");

        let dest = scratch_dir("nested-folders");
        let created =
            export_workspace_to_markdown(&workspace, &dest, TimeFormat::TwelveHour).unwrap();

        assert_eq!(entry_names(&created), vec!["Projects"]);
        assert_eq!(entry_names(&created.join("Projects")), vec!["Plan.md"]);
        let content = fs::read_to_string(created.join("Projects/Plan.md")).unwrap();
        assert_eq!(content, "hello\n");
    }

    #[test]
    fn checkbox_state_and_due_date_render_as_gfm_and_an_annotation_line() {
        let mut item = Item::new("Ship it");
        item.marker = ItemMarker::Checkbox;
        item.state = vec![OccurrenceState {
            occurrence: OccurrenceId::Single,
            state: ItemState {
                progress: -1,
                notification_offset_secs: None,
            },
        }];
        item.end = Some("2020-01-01T17:00:00Z".parse().unwrap());

        let mut out = String::new();
        render_item_block(
            &item,
            TimeFormat::TwentyFourHour,
            0,
            Path::new("/unused"),
            &mut HashSet::new(),
            &mut out,
        )
        .unwrap();

        let mut lines = out.lines();
        assert_eq!(lines.next(), Some("- [x] Ship it"));
        assert!(lines.next().unwrap().contains("due"));
    }

    #[test]
    fn italic_delimiters_become_single_underscore_for_gfm() {
        assert_eq!(
            fix_italic_delimiters("plain __italic__ and **bold**"),
            "plain _italic_ and **bold**"
        );
    }

    #[test]
    fn table_renders_as_a_gfm_pipe_table() {
        let mut table = Table::new(1, 2);
        table.columns[0].name = "Task".to_string();
        table.columns[1].name = "Owner".to_string();
        table.rows[0].cells[0] = TableCell::with_text("Ship it");
        table.rows[0].cells[1] = TableCell::with_text("Ann | Bea");

        let rendered = render_table_markdown(&table);
        let mut lines = rendered.lines();
        assert_eq!(lines.next(), Some("| Task | Owner |"));
        assert_eq!(lines.next(), Some("| --- | --- |"));
        assert_eq!(lines.next(), Some("| Ship it | Ann \\| Bea |"));
    }
}
