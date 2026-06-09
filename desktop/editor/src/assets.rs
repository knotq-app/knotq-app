use std::path::PathBuf;

use uuid::Uuid;

fn data_dir() -> PathBuf {
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
        let home = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        {
            return home.join("Library/Application Support/KnotQ");
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            return home.join(".local/share/knotq");
        }
    }
    PathBuf::from(".")
}

fn workspace_dir() -> PathBuf {
    data_dir().join("workspace")
}

pub(crate) fn image_asset_path(asset: Uuid, extension: &str) -> PathBuf {
    workspace_dir()
        .join("assets")
        .join("images")
        .join(format!("{asset}.{extension}"))
}
