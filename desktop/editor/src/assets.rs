use std::path::PathBuf;

use uuid::Uuid;

fn data_dir() -> PathBuf {
    // Explicit override, used to point a build at a throwaway/seeded data dir
    // without touching real user data. Must stay in sync with
    // `storage-json`'s `data_dir` so image assets land in the same workspace the
    // rest of the app loads, saves, and syncs from.
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
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    if let Some(dir) = linux_data_dir(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME")) {
        return dir;
    }

    #[cfg(not(target_os = "windows"))]
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        {
            return home.join("Library/Application Support/KnotQ");
        }
    }
    PathBuf::from(".")
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn linux_data_dir(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    xdg.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| path.join("knotq"))
        .or_else(|| {
            home.map(PathBuf::from)
                .map(|path| path.join(".local/share/knotq"))
        })
}

#[cfg(all(test, not(target_os = "macos"), not(target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn linux_asset_path_honors_nonempty_xdg_home() {
        assert_eq!(
            linux_data_dir(Some("/xdg".into()), Some("/home/user".into())),
            Some(PathBuf::from("/xdg/knotq"))
        );
    }
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
