use chrono::{Datelike, NaiveDate};
use std::path::PathBuf;
use uuid::Uuid;

pub fn workspace_path() -> PathBuf {
    data_dir().join("workspace.json")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn data_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        {
            return home.join("Library/Application Support/KnotQ");
        }
        #[cfg(not(target_os = "macos"))]
        {
            return home.join(".local/share/knotq");
        }
    }
    PathBuf::from(".")
}

pub fn image_assets_dir() -> PathBuf {
    data_dir().join("assets/images")
}

pub fn image_asset_path(asset: Uuid, extension: &str) -> PathBuf {
    image_assets_dir().join(format!("{asset}.{extension}"))
}

pub(crate) fn schemes_dir(base_dir: &std::path::Path) -> PathBuf {
    base_dir.join("schemes")
}

pub(crate) fn daily_queue_file_path(base_dir: &std::path::Path, date: NaiveDate) -> PathBuf {
    base_dir
        .join("daily_queue")
        .join(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()))
        .join(format!("{:02}.knotq", date.day()))
}
