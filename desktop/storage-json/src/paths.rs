use chrono::{Datelike, NaiveDate};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
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
            let current = home.join("Library/Application Support/KnotQ");
            let legacy = home.join("Library/Application Support/Nutq");
            migrate_legacy_data_dir(&legacy, &current);
            return current;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let current = home.join(".local/share/knotq");
            let legacy = home.join(".local/share/nutq");
            migrate_legacy_data_dir(&legacy, &current);
            return current;
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

pub(crate) fn scheme_file_path(base_dir: &std::path::Path, id: knotq_model::SchemeId) -> PathBuf {
    base_dir.join("schemes").join(format!("{id}.json"))
}

pub(crate) fn daily_queue_file_path(base_dir: &std::path::Path, date: NaiveDate) -> PathBuf {
    base_dir
        .join("daily_queue")
        .join(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()))
        .join(format!("{:02}.knotq", date.day()))
}

pub(crate) fn legacy_daily_queue_file_path(base_dir: &Path, date: NaiveDate) -> PathBuf {
    base_dir
        .join("daily_queue")
        .join(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()))
        .join(format!("{:02}.nutq", date.day()))
}

fn migrate_legacy_data_dir(legacy: &Path, current: &Path) {
    if current.exists() || !legacy.exists() {
        return;
    }
    if let Err(err) = copy_dir_all(legacy, current) {
        eprintln!(
            "KnotQ data migration from {} to {} failed: {err}",
            legacy.display(),
            current.display()
        );
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else if ty.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
