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
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    if let Some(dir) = linux_data_dir(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME")) {
        return dir;
    }

    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join("Library/Application Support/KnotQ");
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
    fn linux_data_path_honors_nonempty_xdg_home() {
        assert_eq!(
            linux_data_dir(Some("/xdg".into()), Some("/home/user".into())),
            Some(PathBuf::from("/xdg/knotq"))
        );
        assert_eq!(
            linux_data_dir(Some("".into()), Some("/home/user".into())),
            Some(PathBuf::from("/home/user/.local/share/knotq"))
        );
    }
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
