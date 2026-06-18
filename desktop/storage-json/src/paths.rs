use std::path::PathBuf;
use uuid::Uuid;

pub fn workspace_path() -> PathBuf {
    workspace_dir().join("workspace.json")
}

pub fn workspace_dir() -> PathBuf {
    data_dir().join("workspace")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn data_dir() -> PathBuf {
    // Explicit override, used to point a build at a throwaway/seeded data dir
    // (e.g. the website screenshot seed) without touching the real user data.
    // launchd resets `HOME`, so this is the reliable way to redirect a bundled
    // app via `LSEnvironment`/`open --env`.
    if let Ok(dir) = std::env::var("KNOTQ_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("KnotQ");
        }
        if let Ok(app_data) = std::env::var("APPDATA") {
            return PathBuf::from(app_data).join("KnotQ");
        }
        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            return PathBuf::from(user_profile).join("AppData/Local/KnotQ");
        }
    }

    #[cfg(not(target_os = "windows"))]
    if let Ok(home) = std::env::var("HOME") {
        #[cfg(target_os = "macos")]
        {
            let home = PathBuf::from(home);
            return home.join("Library/Application Support/KnotQ");
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            let home = PathBuf::from(home);
            return home.join(".local/share/knotq");
        }
    }
    PathBuf::from(".")
}

pub fn image_assets_dir() -> PathBuf {
    workspace_dir().join("assets/images")
}

pub fn image_asset_path(asset: Uuid, extension: &str) -> PathBuf {
    image_assets_dir().join(format!("{asset}.{extension}"))
}

pub(crate) fn schemes_dir(base_dir: &std::path::Path) -> PathBuf {
    base_dir.join("schemes")
}
