use std::path::PathBuf;

use uuid::Uuid;

fn data_dir() -> PathBuf {
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

pub(crate) fn image_asset_path(asset: Uuid, extension: &str) -> PathBuf {
    data_dir()
        .join("assets/images")
        .join(format!("{asset}.{extension}"))
}
