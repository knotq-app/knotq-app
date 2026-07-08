use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use semver::Version;
use serde::Deserialize;

mod download;
mod install;
mod manifest;

use download::{download_asset, verify_asset};
use install::{check_dependencies, install_restart_update};
use manifest::{
    fetch_manifest, matching_asset, parse_version, target_kind, update_manifest_url,
    validate_asset_kind, version_is_newer,
};

const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/knotq-app/knotq-app/releases/latest/download/latest.json";
const USER_AGENT: &str = concat!("KnotQ/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
pub struct AutoUpdateConfig {
    pub current_version: Version,
    pub update_manifest_url: String,
    pub user_agent: String,
}

impl AutoUpdateConfig {
    pub fn github(current_version: Version) -> Self {
        Self {
            current_version,
            update_manifest_url: update_manifest_url(),
            user_agent: USER_AGENT.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LatestRelease {
    UpToDate {
        version: Version,
        release_url: String,
    },
    Available(AvailableUpdate),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableUpdate {
    pub version: Version,
    pub tag_name: String,
    pub release_url: String,
    pub asset: ReleaseAsset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseAsset {
    pub os: String,
    pub arch: String,
    pub kind: String,
    pub name: String,
    pub download_url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallStrategy {
    InstalledOnRestart,
    RunInstallerAndQuit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedUpdate {
    pub version: Version,
    pub tag_name: String,
    pub release_url: String,
    pub asset_name: String,
    pub asset_path: PathBuf,
    pub restart_path: Option<PathBuf>,
    pub install_strategy: InstallStrategy,
    pub staged_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetKind {
    MacOs,
    Linux,
    Windows,
}

#[derive(Debug, Deserialize)]
struct UpdateManifest {
    version: String,
    #[serde(default)]
    notes_url: Option<String>,
    assets: Vec<ManifestAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct ManifestAsset {
    os: String,
    arch: String,
    kind: String,
    name: String,
    url: String,
    sha256: String,
}

pub fn current_version(version: &str) -> Result<Version> {
    parse_version(version).with_context(|| format!("parse current app version {version:?}"))
}

pub fn check_latest_release(config: &AutoUpdateConfig) -> Result<LatestRelease> {
    let manifest = fetch_manifest(config)?;
    let version = parse_version(&manifest.version)
        .with_context(|| format!("parse latest update version {:?}", manifest.version))?;
    let release_url = manifest
        .notes_url
        .clone()
        .unwrap_or_else(|| config.update_manifest_url.clone());

    if !version_is_newer(&config.current_version, &version) {
        return Ok(LatestRelease::UpToDate {
            version,
            release_url,
        });
    }

    let asset = matching_asset(&manifest).cloned().with_context(|| {
        format!(
            "no update asset for {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    validate_asset_kind(&asset)?;

    Ok(LatestRelease::Available(AvailableUpdate {
        version,
        tag_name: format!("v{}", manifest.version),
        release_url,
        asset: ReleaseAsset {
            os: asset.os,
            arch: asset.arch,
            kind: asset.kind,
            name: asset.name,
            download_url: asset.url,
            sha256: asset.sha256.trim().to_ascii_lowercase(),
        },
    }))
}

pub fn check_update_newer_than(
    config: &AutoUpdateConfig,
    version: &Version,
) -> Result<Option<AvailableUpdate>> {
    match check_latest_release(config)? {
        LatestRelease::Available(update) if version_is_newer(version, &update.version) => {
            Ok(Some(update))
        }
        _ => Ok(None),
    }
}

pub fn refresh_available_update(
    config: &AutoUpdateConfig,
    selected: &AvailableUpdate,
) -> Result<AvailableUpdate> {
    Ok(check_update_newer_than(config, &selected.version)?.unwrap_or_else(|| selected.clone()))
}

pub fn prepare_update(
    config: &AutoUpdateConfig,
    update: &AvailableUpdate,
    _app_path: &Path,
    updates_dir: &Path,
) -> Result<StagedUpdate> {
    check_dependencies()?;

    fs::create_dir_all(updates_dir)
        .with_context(|| format!("create updates directory {}", updates_dir.display()))?;
    let version_dir = updates_dir.join(update.version.to_string());
    if version_dir.exists() {
        fs::remove_dir_all(&version_dir)
            .with_context(|| format!("clear old updates directory {}", version_dir.display()))?;
    }
    fs::create_dir_all(&version_dir)
        .with_context(|| format!("create version updates directory {}", version_dir.display()))?;

    let file_name = Path::new(&update.asset.name)
        .file_name()
        .context("update asset name must be a file name")?;
    let asset_path = version_dir.join(file_name);
    download_asset(config, &update.asset, &asset_path)?;
    verify_asset(&asset_path, &update.asset.sha256)?;

    let install_strategy = match target_kind()? {
        TargetKind::MacOs | TargetKind::Linux => InstallStrategy::InstalledOnRestart,
        TargetKind::Windows => InstallStrategy::RunInstallerAndQuit,
    };

    Ok(StagedUpdate {
        version: update.version.clone(),
        tag_name: update.tag_name.clone(),
        release_url: update.release_url.clone(),
        asset_name: update.asset.name.clone(),
        asset_path,
        restart_path: None,
        install_strategy,
        staged_at: Utc::now(),
    })
}

pub fn install_staged_update(update: &StagedUpdate) -> Result<Option<PathBuf>> {
    let download_dir = update
        .asset_path
        .parent()
        .ok_or_else(|| anyhow!("downloaded update path has no parent"))?;

    match update.install_strategy {
        InstallStrategy::InstalledOnRestart => {
            install_restart_update(&update.asset_path, download_dir)
        }
        InstallStrategy::RunInstallerAndQuit => {
            run_windows_installer(update)?;
            Ok(None)
        }
    }
}

pub fn run_windows_installer(update: &StagedUpdate) -> Result<()> {
    if update.install_strategy != InstallStrategy::RunInstallerAndQuit {
        bail!("staged update is not a Windows installer");
    }

    #[cfg(target_os = "windows")]
    {
        let app_exe = std::env::current_exe().context("resolve current executable")?;
        let app_working_dir =
            std::env::current_dir().context("resolve current working directory")?;
        let script = format!(
            r#"
$pidToWaitFor = {}
$installer = '{}'
$appExe = '{}'
$appWorkingDir = '{}'
Wait-Process -Id $pidToWaitFor -ErrorAction SilentlyContinue
$install = Start-Process -FilePath $installer -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/MERGETASKS=!desktopicon') -Wait -PassThru
if ($install.ExitCode -eq 0 -and (Test-Path -LiteralPath $appExe)) {{
    Start-Process -FilePath $appExe -WorkingDirectory $appWorkingDir
}}
"#,
            std::process::id(),
            install::powershell_string(&update.asset_path),
            install::powershell_string(&app_exe),
            install::powershell_string(&app_working_dir)
        );

        Command::new("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command"])
            .arg(script)
            .spawn()
            .with_context(|| format!("launch installer {}", update.asset_path.display()))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        bail!("running Windows installers is not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::matching_asset_for;

    #[test]
    fn parses_v_prefixed_versions() {
        assert_eq!(parse_version("v1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(parse_version("1.2.3").unwrap(), Version::new(1, 2, 3));
    }

    #[test]
    fn newer_version_ignores_prerelease_and_build_metadata() {
        assert!(!version_is_newer(
            &Version::parse("1.2.3").unwrap(),
            &Version::parse("1.2.3+build.1").unwrap()
        ));
        assert!(version_is_newer(
            &Version::parse("1.2.3").unwrap(),
            &Version::parse("1.2.4-beta.1").unwrap()
        ));
    }

    #[test]
    fn matching_asset_accepts_arch_aliases() {
        let manifest = UpdateManifest {
            version: "1.2.3".into(),
            notes_url: None,
            assets: vec![ManifestAsset {
                os: "macos".into(),
                arch: "arm64".into(),
                kind: "dmg".into(),
                name: "KnotQ-1.2.3-macos-arm64.dmg".into(),
                url: "https://example.test/KnotQ.dmg".into(),
                sha256: "a".repeat(64),
            }],
        };

        assert!(matching_asset_for(&manifest, "macos", "aarch64").is_some());
    }

    #[test]
    fn update_newer_than_uses_normalized_semver_ordering() {
        let current = Version::parse("1.2.3+build.1").unwrap();
        let same = Version::parse("1.2.3").unwrap();
        let newer = Version::parse("1.2.4-beta.1").unwrap();

        assert!(!version_is_newer(&current, &same));
        assert!(version_is_newer(&current, &newer));
    }

    #[test]
    fn rejects_invalid_sha256_values() {
        let path = std::env::temp_dir().join(format!(
            "knotq-update-test-{}-{}.bin",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::write(&path, b"test").unwrap();
        let result = verify_asset(&path, "not-a-sha");
        let _ = fs::remove_file(path);
        assert!(result.is_err());
    }
}
