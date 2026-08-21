use super::*;
use std::path::Path;

/// Bring the data directory up to this build's format. Runs exactly once per
/// process, before anything reads or writes user data — both entry points below
/// call it, and either may come first.
fn upgraded_data_directory() -> &'static UpgradeReport {
    static REPORT: std::sync::OnceLock<UpgradeReport> = std::sync::OnceLock::new();
    REPORT.get_or_init(|| {
        let report = run_pending_upgrades(&workspace_path());
        if let Some(line) = report.log_line() {
            eprintln!("{line}");
        }
        report
    })
}

/// Set when the data directory belongs to a build newer than this one. Both the
/// workspace and the settings then load read-only: this build cannot represent
/// what the newer one wrote, so saving would quietly discard it.
fn newer_build_reason() -> Option<String> {
    upgraded_data_directory()
        .written_by_newer_build
        .map(|version| {
            format!(
                "this data directory was written by a newer version of KnotQ \
                 (layout {version}, this build understands {DATA_LAYOUT_VERSION})"
            )
        })
}

pub(crate) struct SettingsBootstrapResult {
    pub(crate) settings: AppSettings,
    pub(crate) save_blocked_reason: Option<String>,
}

pub(crate) fn load_settings_bootstrap() -> SettingsBootstrapResult {
    let path = settings_path();
    let bootstrap = load_settings_or_recover(&path);
    SettingsBootstrapResult {
        settings: bootstrap.settings,
        save_blocked_reason: bootstrap.save_blocked_reason.or_else(newer_build_reason),
    }
}

pub fn load_or_default_settings() -> AppSettings {
    load_settings_bootstrap().settings
}

pub(crate) struct WorkspaceBootstrap {
    pub(crate) workspace: Workspace,
    pub(crate) save_blocked_reason: Option<String>,
}

pub fn load_or_seed() -> WorkspaceBootstrap {
    let path = workspace_path();
    let today = Local::now().date_naive();
    let mut bootstrap = load_or_seed_from_path(&path, today);
    // A newer build's data directory blocks saving even when the workspace file
    // itself parsed: the parts this build does not understand live elsewhere in
    // the directory, and a save rewrites the lot.
    if bootstrap.save_blocked_reason.is_none() {
        bootstrap.save_blocked_reason = newer_build_reason();
    }
    bootstrap
}

fn load_or_seed_from_path(path: &Path, today: NaiveDate) -> WorkspaceBootstrap {
    let options = WorkspaceLoadOptions::daily_queue_range(daily_queue_initial_start(today), today);
    match load_workspace_with_options(path, options) {
        Ok(Some(mut workspace)) => {
            let folders_changed = workspace.normalize_one_level_folders();
            let markers_changed = workspace.normalize_item_markers();
            if folders_changed || markers_changed {
                if let Err(err) = save_workspace(path, &workspace) {
                    eprintln!("workspace repair save failed: {err:#}");
                }
            }
            WorkspaceBootstrap {
                workspace,
                save_blocked_reason: None,
            }
        }
        Ok(None) => {
            let workspace = make_default_workspace_for_date(today);
            if let Err(err) = save_workspace(path, &workspace) {
                eprintln!("initial workspace save failed: {err:#}");
            }
            WorkspaceBootstrap {
                workspace,
                save_blocked_reason: None,
            }
        }
        Err(err) => {
            let reason = format!("{err:#}");
            eprintln!(
                "workspace load failed ({reason}); using default workspace with saving disabled"
            );
            WorkspaceBootstrap {
                workspace: make_default_workspace_for_date(today),
                save_blocked_reason: Some(reason),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    #[test]
    fn failed_workspace_load_blocks_saving_seeded_default() {
        let dir = unique_temp_dir("knotq-bootstrap-load-error");
        let path = dir.join("workspace.json");
        let today = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
        let workspace = make_default_workspace_for_date(today);
        save_workspace(&path, &workspace).unwrap();
        let raw =
            fs::read_to_string(&path)
                .unwrap()
                .replacen("\"version\": 1", "\"version\": 999", 1);
        fs::write(&path, raw).unwrap();

        let bootstrap = load_or_seed_from_path(&path, today);
        assert!(bootstrap.save_blocked_reason.is_some());
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("\"version\": 999"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn successful_workspace_load_persists_startup_normalization() {
        let dir = unique_temp_dir("knotq-bootstrap-normalize-save");
        let path = dir.join("workspace.json");
        let today = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let mut workspace = Workspace::new();
        let active = Scheme::new("Active", 0);
        let active_id = active.id;
        let deleted = Scheme::new("Archived", 1);
        let deleted_id = deleted.id;
        workspace.schemes.insert(active_id, active);
        workspace.schemes.insert(deleted_id, deleted);
        workspace.mark_scheme_deleted_from(deleted_id, workspace.root, 1);
        workspace.folders.get_mut(&workspace.root).unwrap().children =
            vec![NodeRef::Scheme(active_id), NodeRef::Scheme(deleted_id)];
        save_workspace(&path, &workspace).unwrap();

        let bootstrap = load_or_seed_from_path(&path, today);
        assert!(bootstrap.save_blocked_reason.is_none());
        assert_eq!(
            bootstrap
                .workspace
                .folder(bootstrap.workspace.root)
                .unwrap()
                .children,
            vec![NodeRef::Scheme(active_id)]
        );

        let persisted = load_workspace_with_options(&path, WorkspaceLoadOptions::all())
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted.folder(persisted.root).unwrap().children,
            vec![NodeRef::Scheme(active_id)]
        );
        assert!(persisted.is_scheme_deleted(deleted_id));

        let _ = fs::remove_dir_all(dir);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
