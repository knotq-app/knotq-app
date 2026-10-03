use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};

mod download;
mod install;
mod manifest;

use download::{download_asset, verify_asset};
use install::{check_dependencies, install_restart_update, stage_payload};
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
    /// The unpacked payload (the new `.app` bundle on macOS, the extracted tree
    /// on Linux), ready to be copied into place. `None` on Windows, where the
    /// downloaded installer *is* the payload.
    pub payload_path: Option<PathBuf>,
    pub restart_path: Option<PathBuf>,
    pub install_strategy: InstallStrategy,
    pub staged_at: DateTime<Utc>,
}

impl StagedUpdate {
    /// Whether this update is still installable: the payload it would copy from
    /// has to actually be on disk. A user who clears the data directory between
    /// staging and restarting gets a re-download rather than a failed install.
    pub fn payload_is_present(&self) -> bool {
        match &self.payload_path {
            Some(payload) => payload.exists(),
            None => self.asset_path.is_file(),
        }
    }
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

/// Download and unpack an update so that installing it later costs nothing but a
/// local file copy.
///
/// Everything expensive happens here: the download, the checksum, and unpacking
/// the archive. The step the user waits on — [`install_staged_update`] — then
/// only copies the staged payload over the installed one.
///
/// Already staged on a previous run? Then this returns immediately. Nothing is
/// re-downloaded, because the whole point of staging early is that the app can
/// be relaunched any number of times before the user takes the update.
pub fn prepare_update(
    config: &AutoUpdateConfig,
    update: &AvailableUpdate,
    app_path: &Path,
    updates_dir: &Path,
) -> Result<StagedUpdate> {
    check_dependencies()?;

    fs::create_dir_all(updates_dir)
        .with_context(|| format!("create updates directory {}", updates_dir.display()))?;
    let version_dir = updates_dir.join(update.version.to_string());
    let install_strategy = match target_kind()? {
        TargetKind::MacOs | TargetKind::Linux => InstallStrategy::InstalledOnRestart,
        TargetKind::Windows => InstallStrategy::RunInstallerAndQuit,
    };

    if let Some(staged) = reuse_staged_update(&version_dir, update, &install_strategy) {
        discard_superseded_downloads(updates_dir, &version_dir);
        return Ok(staged);
    }

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

    let payload_path = stage_payload(&asset_path, &version_dir, app_path)?;
    if payload_path.is_some() {
        // The archive was only an input to the payload. Keeping it would double
        // what a staged update costs on disk, and nothing reads it again.
        if let Err(err) = fs::remove_file(&asset_path) {
            eprintln!(
                "could not remove unpacked update archive {}: {err}",
                asset_path.display()
            );
        }
    }

    let staged = StagedUpdate {
        version: update.version.clone(),
        tag_name: update.tag_name.clone(),
        release_url: update.release_url.clone(),
        asset_name: update.asset.name.clone(),
        asset_path,
        payload_path,
        restart_path: None,
        install_strategy,
        staged_at: Utc::now(),
    };

    // Written last on purpose: this file is what marks the staging as finished,
    // so a crash part-way through unpacking leaves no manifest and the next run
    // starts over instead of installing half a bundle.
    write_staged_manifest(&version_dir, &staged, &update.asset.sha256);

    // Only now that the new download is on disk and its hash checks out: every
    // previous version's download is dead weight, and nothing ever removed them.
    // Observed at 560 MB of installers in one user's data directory — larger
    // than everything they had actually written.
    discard_superseded_downloads(updates_dir, &version_dir);

    Ok(staged)
}

/// What a finished staging wrote next to the payload, so a later run can pick it
/// up instead of downloading the same release again.
#[derive(Debug, Deserialize, Serialize)]
struct StagedManifest {
    version: String,
    tag_name: String,
    release_url: String,
    asset_name: String,
    /// The digest of the release asset the payload was unpacked from. A manifest
    /// whose digest no longer matches the release is not the release any more.
    asset_sha256: String,
    asset_path: PathBuf,
    payload_path: Option<PathBuf>,
    staged_at: DateTime<Utc>,
}

const STAGED_MANIFEST_FILE: &str = "staged.json";

fn write_staged_manifest(version_dir: &Path, staged: &StagedUpdate, asset_sha256: &str) {
    let manifest = StagedManifest {
        version: staged.version.to_string(),
        tag_name: staged.tag_name.clone(),
        release_url: staged.release_url.clone(),
        asset_name: staged.asset_name.clone(),
        asset_sha256: asset_sha256.trim().to_ascii_lowercase(),
        asset_path: staged.asset_path.clone(),
        payload_path: staged.payload_path.clone(),
        staged_at: staged.staged_at,
    };
    let path = version_dir.join(STAGED_MANIFEST_FILE);
    match serde_json::to_vec_pretty(&manifest) {
        Ok(bytes) => {
            if let Err(err) = fs::write(&path, bytes) {
                eprintln!("could not record staged update {}: {err}", path.display());
            }
        }
        Err(err) => eprintln!("could not encode staged update {}: {err}", path.display()),
    }
}

/// An update already staged for this exact release, if one is on disk.
///
/// Deliberately conservative: anything unexpected returns `None` and the caller
/// stages again from scratch. A needless re-download is a slow update; installing
/// a payload that does not match the release is a broken one.
fn reuse_staged_update(
    version_dir: &Path,
    update: &AvailableUpdate,
    install_strategy: &InstallStrategy,
) -> Option<StagedUpdate> {
    let raw = fs::read(version_dir.join(STAGED_MANIFEST_FILE)).ok()?;
    let manifest: StagedManifest = serde_json::from_slice(&raw).ok()?;
    if manifest.asset_sha256 != update.asset.sha256.trim().to_ascii_lowercase() {
        return None;
    }
    if manifest.version != update.version.to_string() {
        return None;
    }

    let staged = StagedUpdate {
        version: update.version.clone(),
        tag_name: manifest.tag_name,
        release_url: manifest.release_url,
        asset_name: manifest.asset_name,
        asset_path: manifest.asset_path,
        payload_path: manifest.payload_path,
        restart_path: None,
        install_strategy: install_strategy.clone(),
        staged_at: manifest.staged_at,
    };
    staged.payload_is_present().then_some(staged)
}

/// Remove staged downloads for releases this build is no longer behind.
///
/// Once an update is installed, the payload it was installed from is dead weight
/// — and it is by far the largest thing the app ever writes. Nothing else would
/// ever remove it: [`prepare_update`] only sweeps what it supersedes, and an app
/// that is up to date never calls one, so the payload for the release now running
/// would sit in the data directory indefinitely.
///
/// Best effort, and deliberately narrow: only directories named after a version
/// that is not ahead of `current_version` are touched.
pub fn discard_installed_downloads(updates_dir: &Path, current_version: &Version) {
    let Ok(entries) = fs::read_dir(updates_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(version) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| parse_version(name).ok())
        else {
            continue;
        };
        if version_is_newer(current_version, &version) {
            continue;
        }
        if let Err(err) = fs::remove_dir_all(&path) {
            eprintln!(
                "could not remove installed update {}: {err}",
                path.display()
            );
        }
    }
}

/// Remove every staged download except `keep`.
///
/// Deliberately after the download succeeds rather than before it: a failed
/// download must not also cost the user the update they already have staged and
/// verified. Best effort — a leftover directory is only wasted space, and a
/// failure here must never fail an update.
fn discard_superseded_downloads(updates_dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(updates_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || path == keep {
            continue;
        }
        if let Err(err) = fs::remove_dir_all(&path) {
            eprintln!(
                "could not remove superseded update {}: {err}",
                path.display()
            );
        }
    }
}

pub fn install_staged_update(update: &StagedUpdate) -> Result<Option<PathBuf>> {
    let download_dir = update
        .asset_path
        .parent()
        .ok_or_else(|| anyhow!("downloaded update path has no parent"))?;

    match update.install_strategy {
        InstallStrategy::InstalledOnRestart => install_restart_update(
            update.payload_path.as_deref(),
            &update.asset_path,
            download_dir,
        ),
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

    fn available(version: &str, sha256: &str) -> AvailableUpdate {
        AvailableUpdate {
            version: Version::parse(version).unwrap(),
            tag_name: format!("v{version}"),
            release_url: "https://example.test/notes".into(),
            asset: ReleaseAsset {
                os: "macos".into(),
                arch: "aarch64".into(),
                kind: "dmg".into(),
                name: format!("KnotQ-{version}-macos-arm64.dmg"),
                download_url: "https://example.test/KnotQ.dmg".into(),
                sha256: sha256.into(),
            },
        }
    }

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "knotq-{label}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write out a finished staging of `update`, payload included.
    fn stage_fixture(version_dir: &Path, update: &AvailableUpdate) -> StagedUpdate {
        let payload = version_dir.join("payload").join("KnotQ.app");
        fs::create_dir_all(&payload).unwrap();
        fs::write(payload.join("Info.plist"), b"staged").unwrap();
        let staged = StagedUpdate {
            version: update.version.clone(),
            tag_name: update.tag_name.clone(),
            release_url: update.release_url.clone(),
            asset_name: update.asset.name.clone(),
            asset_path: version_dir.join(&update.asset.name),
            payload_path: Some(payload),
            restart_path: None,
            install_strategy: InstallStrategy::InstalledOnRestart,
            staged_at: Utc::now(),
        };
        write_staged_manifest(version_dir, &staged, &update.asset.sha256);
        staged
    }

    /// The point of staging an update before the user asks for it: relaunching
    /// before taking it must not throw the download away. Every launch checks for
    /// updates, so without this the same release is fetched again every time.
    #[test]
    fn an_already_staged_release_is_reused_without_downloading_again() {
        let updates_dir = scratch_dir("updates-reuse");
        let update = available("0.60.0", &"b".repeat(64));
        let version_dir = updates_dir.join("0.60.0");
        fs::create_dir_all(&version_dir).unwrap();
        let staged = stage_fixture(&version_dir, &update);

        let reused =
            reuse_staged_update(&version_dir, &update, &InstallStrategy::InstalledOnRestart)
                .expect("staged update should be reused");

        assert_eq!(reused.payload_path, staged.payload_path);
        assert_eq!(reused.version, update.version);
        assert_eq!(reused.asset_name, update.asset.name);
        assert_eq!(reused.staged_at, staged.staged_at);

        let _ = fs::remove_dir_all(&updates_dir);
    }

    /// A tag can be re-cut with the same version number. The digest is what says
    /// whether the payload on disk is the release being offered.
    #[test]
    fn a_staged_payload_from_a_different_asset_is_not_reused() {
        let updates_dir = scratch_dir("updates-digest");
        let version_dir = updates_dir.join("0.60.0");
        fs::create_dir_all(&version_dir).unwrap();
        stage_fixture(&version_dir, &available("0.60.0", &"b".repeat(64)));

        let republished = available("0.60.0", &"c".repeat(64));
        assert!(reuse_staged_update(
            &version_dir,
            &republished,
            &InstallStrategy::InstalledOnRestart
        )
        .is_none());

        let _ = fs::remove_dir_all(&updates_dir);
    }

    /// The manifest is written last, so a staging that died part-way through
    /// leaves a half-copied payload and no manifest. Installing that would
    /// produce a broken app bundle.
    #[test]
    fn an_unfinished_staging_is_not_reused() {
        let updates_dir = scratch_dir("updates-unfinished");
        let update = available("0.60.0", &"b".repeat(64));
        let version_dir = updates_dir.join("0.60.0");
        let payload = version_dir.join("payload").join("KnotQ.app");
        fs::create_dir_all(&payload).unwrap();
        fs::write(payload.join("Info.plist"), b"half a bundle").unwrap();

        assert!(
            reuse_staged_update(&version_dir, &update, &InstallStrategy::InstalledOnRestart)
                .is_none()
        );

        let _ = fs::remove_dir_all(&updates_dir);
    }

    /// Whatever the manifest says, the payload has to still be there — the data
    /// directory it lives in is one a user may clear out.
    #[test]
    fn a_staged_update_whose_payload_is_gone_is_not_reused() {
        let updates_dir = scratch_dir("updates-vanished");
        let update = available("0.60.0", &"b".repeat(64));
        let version_dir = updates_dir.join("0.60.0");
        fs::create_dir_all(&version_dir).unwrap();
        let staged = stage_fixture(&version_dir, &update);
        fs::remove_dir_all(staged.payload_path.as_ref().unwrap()).unwrap();

        assert!(!staged.payload_is_present());
        assert!(
            reuse_staged_update(&version_dir, &update, &InstallStrategy::InstalledOnRestart)
                .is_none()
        );

        let _ = fs::remove_dir_all(&updates_dir);
    }

    /// Windows has no unpacked payload: the downloaded installer is what runs.
    #[test]
    fn a_staged_windows_installer_counts_as_its_own_payload() {
        let updates_dir = scratch_dir("updates-windows");
        let version_dir = updates_dir.join("0.60.0");
        fs::create_dir_all(&version_dir).unwrap();
        let installer = version_dir.join("KnotQ-Setup.exe");
        fs::write(&installer, b"installer").unwrap();
        let staged = StagedUpdate {
            version: Version::parse("0.60.0").unwrap(),
            tag_name: "v0.60.0".into(),
            release_url: "https://example.test/notes".into(),
            asset_name: "KnotQ-Setup.exe".into(),
            asset_path: installer.clone(),
            payload_path: None,
            restart_path: None,
            install_strategy: InstallStrategy::RunInstallerAndQuit,
            staged_at: Utc::now(),
        };

        assert!(staged.payload_is_present());
        fs::remove_file(&installer).unwrap();
        assert!(!staged.payload_is_present());

        let _ = fs::remove_dir_all(&updates_dir);
    }

    /// The payload an update was installed from survives the install, and after
    /// the relaunch nothing else would ever remove it.
    #[test]
    fn downloads_for_releases_already_installed_are_discarded() {
        let updates_dir = scratch_dir("updates-installed");
        for version in ["0.58.0", "0.59.0", "0.60.0", "0.61.0"] {
            let payload = updates_dir.join(version).join("payload/KnotQ.app");
            fs::create_dir_all(&payload).unwrap();
            fs::write(payload.join("Info.plist"), vec![0u8; 1024]).unwrap();
        }
        // Not a version directory, and so none of this function's business.
        fs::create_dir_all(updates_dir.join("mount")).unwrap();
        fs::write(updates_dir.join("notes.txt"), b"keep me").unwrap();

        discard_installed_downloads(&updates_dir, &Version::parse("0.60.0").unwrap());

        let remaining: Vec<String> = fs::read_dir(&updates_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        for installed in ["0.58.0", "0.59.0", "0.60.0"] {
            assert!(
                !remaining.contains(&installed.to_string()),
                "{installed} is already installed but was kept: {remaining:?}"
            );
        }
        assert!(
            remaining.contains(&"0.61.0".to_string()),
            "a release still ahead of this build was discarded: {remaining:?}"
        );
        assert!(remaining.contains(&"mount".to_string()));
        assert!(remaining.contains(&"notes.txt".to_string()));

        let _ = fs::remove_dir_all(&updates_dir);
    }

    /// Every update ever downloaded used to stay on disk forever. Nothing reads
    /// a superseded one, and they are the largest files the app ever writes.
    #[test]
    fn preparing_an_update_discards_the_downloads_it_supersedes() {
        let updates_dir = std::env::temp_dir().join(format!(
            "knotq-updates-sweep-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        for version in ["0.50.0", "0.51.0", "0.52.0"] {
            let dir = updates_dir.join(version);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("KnotQ.dmg"), vec![0u8; 1024]).unwrap();
        }
        // Something that is not a version directory must be left alone.
        fs::write(updates_dir.join("notes.txt"), b"keep me").unwrap();
        let keep = updates_dir.join("0.53.0");
        fs::create_dir_all(&keep).unwrap();
        fs::write(keep.join("KnotQ.dmg"), b"the new one").unwrap();

        discard_superseded_downloads(&updates_dir, &keep);

        let remaining: Vec<String> = fs::read_dir(&updates_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert!(remaining.contains(&"0.53.0".to_string()));
        assert!(remaining.contains(&"notes.txt".to_string()));
        for superseded in ["0.50.0", "0.51.0", "0.52.0"] {
            assert!(
                !remaining.contains(&superseded.to_string()),
                "{superseded} was left behind: {remaining:?}"
            );
        }
        assert_eq!(fs::read(keep.join("KnotQ.dmg")).unwrap(), b"the new one");

        let _ = fs::remove_dir_all(&updates_dir);
    }

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

/// End-to-end exercise of the staging path: a real HTTP server, a real download,
/// a real checksum, and on macOS a real disk image.
///
/// This is what the app runs when it finds an update, and the behaviour that
/// matters is as much about what it *does not* do — download the same release a
/// second time — as about what it produces.
#[cfg(test)]
mod end_to_end_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct ReleaseServer {
        addr: SocketAddr,
        asset_requests: Arc<AtomicUsize>,
        manifest_requests: Arc<AtomicUsize>,
    }

    impl ReleaseServer {
        /// Serve `latest.json` and the asset bytes until the process ends.
        fn start(manifest: String, asset: Vec<u8>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let asset_requests = Arc::new(AtomicUsize::new(0));
            let manifest_requests = Arc::new(AtomicUsize::new(0));
            let counters = (asset_requests.clone(), manifest_requests.clone());

            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let Some(target) = read_request_target(&mut stream) else {
                        continue;
                    };
                    if target.ends_with(".json") {
                        counters.1.fetch_add(1, Ordering::SeqCst);
                        respond(&mut stream, b"application/json", manifest.as_bytes());
                    } else {
                        counters.0.fetch_add(1, Ordering::SeqCst);
                        respond(&mut stream, b"application/octet-stream", &asset);
                    }
                }
            });

            Self {
                addr,
                asset_requests,
                manifest_requests,
            }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}/{path}", self.addr)
        }
    }

    fn read_request_target(stream: &mut TcpStream) -> Option<String> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).ok()?;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).ok()? == 0 || header.trim().is_empty() {
                break;
            }
        }
        request_line.split_whitespace().nth(1).map(str::to_string)
    }

    fn respond(stream: &mut TcpStream, content_type: &[u8], body: &[u8]) {
        let mut head = Vec::new();
        head.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Type: ");
        head.extend_from_slice(content_type);
        head.extend_from_slice(format!("\r\nContent-Length: {}\r\n", body.len()).as_bytes());
        head.extend_from_slice(b"Connection: close\r\n\r\n");
        let _ = stream.write_all(&head);
        let _ = stream.write_all(body);
        let _ = stream.flush();
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "knotq-{label}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The asset the manifest offers, built the way the real release is: a disk
    /// image on macOS, a gzipped tarball on Linux, opaque bytes on Windows.
    fn release_asset(dir: &Path) -> (String, Vec<u8>) {
        #[cfg(target_os = "macos")]
        {
            let stage = dir.join("stage");
            let app = stage.join("KnotQ.app/Contents/MacOS");
            fs::create_dir_all(&app).unwrap();
            fs::write(app.join("KnotQ"), b"the new binary").unwrap();
            fs::write(
                stage.join("KnotQ.app/Contents/Info.plist"),
                b"the new plist",
            )
            .unwrap();
            let dmg = dir.join("KnotQ.dmg");
            let output = std::process::Command::new("hdiutil")
                .args(["create", "-volname", "KnotQ", "-srcfolder"])
                .arg(&stage)
                .args(["-ov", "-format", "UDZO", "-quiet"])
                .arg(&dmg)
                .output()
                .expect("hdiutil create");
            assert!(output.status.success());
            ("KnotQ.dmg".to_string(), fs::read(&dmg).unwrap())
        }
        #[cfg(target_os = "linux")]
        {
            let stage = dir.join("stage");
            fs::create_dir_all(&stage).unwrap();
            fs::write(stage.join("knotq"), b"the new binary").unwrap();
            fs::write(stage.join("knotq-mcp"), b"the new bridge").unwrap();
            let tarball = dir.join("knotq.tar.gz");
            let output = std::process::Command::new("tar")
                .arg("-czf")
                .arg(&tarball)
                .arg("-C")
                .arg(&stage)
                .args(["knotq", "knotq-mcp"])
                .output()
                .expect("tar");
            assert!(output.status.success());
            (
                "knotq-linux.tar.gz".to_string(),
                fs::read(&tarball).unwrap(),
            )
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = dir;
            ("KnotQ-Setup.exe".to_string(), b"installer bytes".to_vec())
        }
    }

    fn asset_kind() -> &'static str {
        match std::env::consts::OS {
            "macos" => "dmg",
            "linux" => "tar.gz",
            _ => "inno",
        }
    }

    /// Download, verify and unpack a release, then ask for the same release again
    /// and confirm nothing is fetched twice.
    #[test]
    fn a_release_is_staged_once_and_then_reused() {
        let _serialized = crate::install::disk_image_tool_lock();
        let root = scratch_dir("end-to-end");
        let (asset_name, asset_bytes) = release_asset(&root);
        let digest = sha256_hex(&asset_bytes);
        let listener_addr = {
            // Bind first so the manifest can name the URL it will be served from.
            let probe = TcpListener::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap()
        };
        let asset_url = format!("http://{listener_addr}/{asset_name}");
        let manifest = format!(
            r#"{{"version":"9.9.9","notes_url":"https://example.test/notes","assets":[
                 {{"os":"{os}","arch":"{arch}","kind":"{kind}","name":"{name}",
                   "url":"{url}","sha256":"{sha}"}}]}}"#,
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            kind = asset_kind(),
            name = asset_name,
            url = asset_url,
            sha = digest,
        );
        let server = ReleaseServer::start(manifest, asset_bytes.clone());
        // The manifest was written for the address probed above; serve from the
        // one actually bound.
        let manifest_url = server.url("latest.json");
        let config = AutoUpdateConfig {
            current_version: Version::parse("0.1.0").unwrap(),
            update_manifest_url: manifest_url,
            user_agent: "KnotQ/test".into(),
        };

        // The asset URL in the manifest has to point at this server, so rebuild
        // the manifest with the real address by restarting on the same bytes.
        let asset_url = server.url(&asset_name);
        let update = AvailableUpdate {
            version: Version::parse("9.9.9").unwrap(),
            tag_name: "v9.9.9".into(),
            release_url: "https://example.test/notes".into(),
            asset: ReleaseAsset {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                kind: asset_kind().into(),
                name: asset_name.clone(),
                download_url: asset_url,
                sha256: digest.clone(),
            },
        };

        let updates_dir = root.join("updates");
        let app_path = root.join("Applications/KnotQ.app");

        let staged = prepare_update(&config, &update, &app_path, &updates_dir).unwrap();

        assert_eq!(staged.version, update.version);
        assert_eq!(server.asset_requests.load(Ordering::SeqCst), 1);
        assert!(staged.payload_is_present());

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let payload = staged.payload_path.as_ref().expect("payload staged");
            assert!(payload.exists(), "payload missing at {}", payload.display());
            assert!(
                !staged.asset_path.exists(),
                "the unpacked archive was left on disk at {}",
                staged.asset_path.display()
            );
            #[cfg(target_os = "macos")]
            assert_eq!(
                fs::read_to_string(payload.join("Contents/MacOS/KnotQ")).unwrap(),
                "the new binary"
            );
            #[cfg(target_os = "linux")]
            assert_eq!(
                fs::read_to_string(payload.join("knotq")).unwrap(),
                "the new binary"
            );
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        assert!(staged.asset_path.is_file());

        // Launching again finds the same release on offer. Nothing may be
        // downloaded or unpacked a second time.
        let again = prepare_update(&config, &update, &app_path, &updates_dir).unwrap();

        assert_eq!(
            server.asset_requests.load(Ordering::SeqCst),
            1,
            "the staged release was downloaded again"
        );
        assert_eq!(again.payload_path, staged.payload_path);
        assert_eq!(again.staged_at, staged.staged_at);
        assert_eq!(again.version, staged.version);
        assert_eq!(again.install_strategy, staged.install_strategy);

        let _ = server.manifest_requests;
        let _ = fs::remove_dir_all(&root);
    }

    /// A release republished under the same version number — the digest changes —
    /// has to be fetched and unpacked again rather than reusing what is staged.
    #[test]
    fn a_republished_release_is_staged_again() {
        let _serialized = crate::install::disk_image_tool_lock();
        let root = scratch_dir("end-to-end-republish");
        let (asset_name, asset_bytes) = release_asset(&root);
        let digest = sha256_hex(&asset_bytes);
        let server = ReleaseServer::start(String::new(), asset_bytes.clone());
        let config = AutoUpdateConfig {
            current_version: Version::parse("0.1.0").unwrap(),
            update_manifest_url: server.url("latest.json"),
            user_agent: "KnotQ/test".into(),
        };
        let update = AvailableUpdate {
            version: Version::parse("9.9.9").unwrap(),
            tag_name: "v9.9.9".into(),
            release_url: "https://example.test/notes".into(),
            asset: ReleaseAsset {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                kind: asset_kind().into(),
                name: asset_name,
                download_url: server.url("asset"),
                sha256: digest,
            },
        };
        let updates_dir = root.join("updates");
        let app_path = root.join("Applications/KnotQ.app");

        prepare_update(&config, &update, &app_path, &updates_dir).unwrap();
        assert_eq!(server.asset_requests.load(Ordering::SeqCst), 1);

        let mut republished = update.clone();
        republished.asset.sha256 = sha256_hex(b"different bytes entirely");
        // The server still serves the original bytes, so the checksum must reject
        // them rather than stage a release that is not the one on offer.
        let error = prepare_update(&config, &republished, &app_path, &updates_dir).unwrap_err();
        assert!(
            format!("{error:#}").contains("checksum mismatch"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            server.asset_requests.load(Ordering::SeqCst),
            2,
            "the republished release was not fetched again"
        );

        let _ = server.manifest_requests;
        let _ = fs::remove_dir_all(&root);
    }

    /// `check_latest_release` against a served manifest: the version comparison,
    /// the asset match for this platform, and the notes URL.
    #[test]
    fn a_served_manifest_offers_the_asset_for_this_platform() {
        // Nothing is downloaded here, so the asset needs no real disk image.
        let root = scratch_dir("end-to-end-manifest");
        let asset_name = "KnotQ-9.9.9".to_string();
        let digest = sha256_hex(b"asset bytes");
        let manifest = format!(
            r#"{{"version":"9.9.9","notes_url":"https://example.test/notes","assets":[
                 {{"os":"{os}","arch":"{arch}","kind":"{kind}","name":"{name}",
                   "url":"https://example.test/{name}","sha256":"{sha}"}}]}}"#,
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            kind = asset_kind(),
            name = asset_name,
            sha = digest.to_uppercase(),
        );
        let server = ReleaseServer::start(manifest, Vec::new());

        let config = AutoUpdateConfig {
            current_version: Version::parse("0.1.0").unwrap(),
            update_manifest_url: server.url("latest.json"),
            user_agent: "KnotQ/test".into(),
        };
        let latest = check_latest_release(&config).unwrap();
        let LatestRelease::Available(update) = latest else {
            panic!("expected an available update, got {latest:?}");
        };
        assert_eq!(update.version, Version::parse("9.9.9").unwrap());
        assert_eq!(update.tag_name, "v9.9.9");
        assert_eq!(update.release_url, "https://example.test/notes");
        assert_eq!(update.asset.sha256, digest, "digest should be normalized");

        // The same manifest against a newer build is not an update.
        let config = AutoUpdateConfig {
            current_version: Version::parse("9.9.9").unwrap(),
            ..config
        };
        assert!(matches!(
            check_latest_release(&config).unwrap(),
            LatestRelease::UpToDate { .. }
        ));
        assert!(
            check_update_newer_than(&config, &Version::parse("9.9.9").unwrap())
                .unwrap()
                .is_none()
        );

        let _ = server.asset_requests;
        let _ = fs::remove_dir_all(&root);
    }
}
