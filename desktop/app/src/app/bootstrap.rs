use super::*;

pub fn load_or_default_settings() -> AppSettings {
    let path = settings_path();
    match load_app_settings(&path) {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("settings load failed ({err:#}); using defaults");
            AppSettings::default()
        }
    }
}

pub fn load_or_seed() -> Workspace {
    let path = workspace_path();
    let today = Local::now().date_naive();
    let options = WorkspaceLoadOptions::daily_queue_range(daily_queue_initial_start(today), today);
    match load_workspace_with_options(&path, options) {
        Ok(Some(ws)) => ws,
        Ok(None) => {
            let ws = make_default_workspace_for_date(today);
            if let Err(err) = save_workspace(&path, &ws) {
                eprintln!("initial workspace save failed: {err:#}");
            }
            ws
        }
        Err(err) => {
            eprintln!("workspace load failed ({err:#}); seeding default workspace");
            make_default_workspace_for_date(today)
        }
    }
}
