use super::*;
use knotq_storage_json::save_app_settings;
use std::path::Path;

pub fn load_or_default_settings() -> AppSettings {
    let path = settings_path();
    match load_app_settings(&path) {
        Ok(settings) => migrate_local_sync_api_base(settings, &path),
        Err(err) => {
            eprintln!("settings load failed ({err:#}); using defaults");
            AppSettings::default()
        }
    }
}

fn migrate_local_sync_api_base(mut settings: AppSettings, path: &Path) -> AppSettings {
    let Some(account) = settings.sync_account.as_mut() else {
        return settings;
    };
    let api_base = account.api_base.trim_end_matches('/');
    if api_base != "http://127.0.0.1:7878" && api_base != "http://localhost:7878" {
        return settings;
    }
    account.api_base = "http://127.0.0.1:8787".to_string();
    if let Err(err) = save_app_settings(path, &settings) {
        eprintln!("sync settings migration failed: {err:#}");
    }
    settings
}

pub(crate) struct WorkspaceBootstrap {
    pub(crate) workspace: Workspace,
    pub(crate) save_blocked_reason: Option<String>,
}

pub fn load_or_seed() -> WorkspaceBootstrap {
    let path = workspace_path();
    let today = Local::now().date_naive();
    load_or_seed_from_path(&path, today)
}

fn load_or_seed_from_path(path: &Path, today: NaiveDate) -> WorkspaceBootstrap {
    let options = WorkspaceLoadOptions::daily_queue_range(daily_queue_initial_start(today), today);
    match load_workspace_with_options(path, options) {
        Ok(Some(workspace)) => WorkspaceBootstrap {
            workspace,
            save_blocked_reason: None,
        },
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
