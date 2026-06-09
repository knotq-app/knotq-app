use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::Command;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Output, Stdio};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{ffi::OsStr, process::Command as PlatformCommand};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use semver::{BuildMetadata, Prerelease, Version};
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use walkdir::WalkDir;

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
            powershell_string(&update.asset_path),
            powershell_string(&app_exe),
            powershell_string(&app_working_dir)
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

fn fetch_manifest(config: &AutoUpdateConfig) -> Result<UpdateManifest> {
    let response = ureq::get(&config.update_manifest_url)
        .set("Accept", "application/json")
        .set("User-Agent", &config.user_agent)
        .call()
        .with_context(|| format!("fetch update manifest from {}", config.update_manifest_url))?;
    response.into_json().context("decode update manifest JSON")
}

fn download_asset(
    config: &AutoUpdateConfig,
    asset: &ReleaseAsset,
    destination: &Path,
) -> Result<()> {
    if destination.is_file() && verify_asset(destination, &asset.sha256).is_ok() {
        return Ok(());
    }

    let tmp_path = destination.with_extension("download");
    let response = ureq::get(&asset.download_url)
        .set("Accept", "application/octet-stream")
        .set("User-Agent", &config.user_agent)
        .call()
        .with_context(|| format!("download update asset {}", asset.name))?;

    let mut tmp_file = File::create(&tmp_path)
        .with_context(|| format!("create temporary download {}", tmp_path.display()))?;
    io::copy(&mut response.into_reader(), &mut tmp_file)
        .with_context(|| format!("write temporary download {}", tmp_path.display()))?;
    tmp_file
        .flush()
        .with_context(|| format!("flush temporary download {}", tmp_path.display()))?;
    fs::rename(&tmp_path, destination)
        .with_context(|| format!("move download into {}", destination.display()))?;
    Ok(())
}

fn verify_asset(path: &Path, expected_sha256: &str) -> Result<()> {
    let expected_sha256 = expected_sha256.trim();
    if !is_sha256_hex(expected_sha256) {
        bail!("update manifest contains invalid SHA-256 digest {expected_sha256:?}");
    }

    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        bail!("downloaded update checksum mismatch: expected {expected_sha256}, got {actual}");
    }

    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn parse_version(raw: &str) -> Result<Version> {
    let trimmed = raw.trim().trim_start_matches('v');
    Version::parse(trimmed).map_err(Into::into)
}

fn version_is_newer(current_version: &Version, fetched_version: &Version) -> bool {
    normalize_version(fetched_version) > normalize_version(current_version)
}

fn normalize_version(version: &Version) -> Version {
    let mut normalized = version.clone();
    normalized.pre = Prerelease::EMPTY;
    normalized.build = BuildMetadata::EMPTY;
    normalized
}

fn update_manifest_url() -> String {
    std::env::var("KNOTQ_UPDATE_MANIFEST_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .or_else(|| option_env!("KNOTQ_UPDATE_MANIFEST_URL").map(str::to_string))
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_string())
}

fn matching_asset(manifest: &UpdateManifest) -> Option<&ManifestAsset> {
    matching_asset_for(manifest, std::env::consts::OS, std::env::consts::ARCH)
}

fn matching_asset_for<'a>(
    manifest: &'a UpdateManifest,
    os: &str,
    arch: &str,
) -> Option<&'a ManifestAsset> {
    manifest
        .assets
        .iter()
        .find(|asset| asset.os == os && arch_matches(&asset.arch, arch))
}

fn arch_matches(asset_arch: &str, current_arch: &str) -> bool {
    asset_arch == current_arch
        || matches!(
            (asset_arch, current_arch),
            ("x86_64", "amd64") | ("aarch64", "arm64") | ("arm64", "aarch64")
        )
}

fn validate_asset_kind(asset: &ManifestAsset) -> Result<()> {
    let expected_kind = match std::env::consts::OS {
        "macos" => "dmg",
        "linux" => "tar.gz",
        "windows" => "inno",
        os => bail!("auto updates are not supported on {os}"),
    };

    if asset.kind != expected_kind {
        bail!(
            "update asset {:?} has kind {:?}, expected {:?}",
            asset.name,
            asset.kind,
            expected_kind
        );
    }

    Ok(())
}

fn target_kind() -> Result<TargetKind> {
    match std::env::consts::OS {
        "macos" => Ok(TargetKind::MacOs),
        "linux" => Ok(TargetKind::Linux),
        "windows" => Ok(TargetKind::Windows),
        os => bail!("auto updates are not supported on {os}"),
    }
}

#[cfg(target_os = "macos")]
fn install_restart_update(downloaded_asset: &Path, download_dir: &Path) -> Result<Option<PathBuf>> {
    install_macos_update(downloaded_asset, download_dir)?;
    Ok(None)
}

#[cfg(target_os = "linux")]
fn install_restart_update(downloaded_asset: &Path, download_dir: &Path) -> Result<Option<PathBuf>> {
    install_linux_update(downloaded_asset, download_dir).map(Some)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_restart_update(
    _downloaded_asset: &Path,
    _download_dir: &Path,
) -> Result<Option<PathBuf>> {
    bail!(
        "restart updates are not supported on {}",
        std::env::consts::OS
    )
}

#[cfg(target_os = "macos")]
fn install_macos_update(downloaded_dmg: &Path, temp_dir: &Path) -> Result<()> {
    let app_path = current_macos_app_bundle()?;
    let app_name = app_path
        .file_name()
        .ok_or_else(|| anyhow!("invalid app bundle path {}", app_path.display()))?;
    let mount_root = temp_dir.join("mount");
    fs::create_dir_all(&mount_root)
        .with_context(|| format!("failed to create {}", mount_root.display()))?;

    let output = PlatformCommand::new("hdiutil")
        .args(["attach", "-nobrowse"])
        .arg(downloaded_dmg)
        .arg("-mountroot")
        .arg(&mount_root)
        .output()
        .context("failed to mount update disk image")?;
    ensure_output_success(output, "mount update disk image")?;

    let (mount_path, mounted_app) = find_mounted_app(&mount_root, app_name)?;
    let copy_result = sync_dir_filtered(&mounted_app, &app_path, macos_finder_icon_file);

    let detach_result = PlatformCommand::new("hdiutil")
        .args(["detach", "-force"])
        .arg(&mount_path)
        .output()
        .context("failed to detach update disk image")
        .and_then(|output| ensure_output_success(output, "detach update disk image"));

    copy_result.and(detach_result)
}

#[cfg(target_os = "macos")]
fn current_macos_app_bundle() -> Result<PathBuf> {
    let current_exe = std::env::current_exe().context("failed to resolve current executable")?;
    current_exe
        .ancestors()
        .find(|path| {
            path.extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension == "app")
        })
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("auto updates require KnotQ to be running from an .app bundle"))
}

#[cfg(target_os = "macos")]
fn find_mounted_app(mount_root: &Path, app_name: &OsStr) -> Result<(PathBuf, PathBuf)> {
    for entry in fs::read_dir(mount_root)
        .with_context(|| format!("failed to read {}", mount_root.display()))?
    {
        let entry = entry?;
        let mount_path = entry.path();
        if !mount_path.is_dir() {
            continue;
        }

        let app = mount_path.join(app_name);
        if app.is_dir() {
            return Ok((mount_path, app));
        }
    }

    bail!(
        "mounted update disk image did not contain {}",
        Path::new(app_name).display()
    )
}

#[cfg(target_os = "macos")]
fn macos_finder_icon_file(relative_path: &Path) -> bool {
    relative_path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.starts_with("Icon") && name.chars().count() == 5
    })
}

#[cfg(target_os = "linux")]
fn install_linux_update(downloaded_tar_gz: &Path, temp_dir: &Path) -> Result<PathBuf> {
    let extracted = temp_dir.join("extract");
    fs::create_dir_all(&extracted)
        .with_context(|| format!("failed to create {}", extracted.display()))?;

    let output = PlatformCommand::new("tar")
        .arg("-xzf")
        .arg(downloaded_tar_gz)
        .arg("-C")
        .arg(&extracted)
        .output()
        .context("failed to extract Linux update")?;
    ensure_output_success(output, "extract Linux update")?;

    let source_exe = extracted.join("knotq");
    if !source_exe.is_file() {
        bail!("Linux update did not contain knotq");
    }

    let current_exe = std::env::current_exe().context("failed to resolve current executable")?;
    let install_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow!("current executable has no parent directory"))?;
    let target_exe = install_dir.join("knotq");
    copy_file(&source_exe, &target_exe)?;

    let source_assets = extracted.join("assets");
    if source_assets.is_dir() {
        sync_dir_filtered(&source_assets, &install_dir.join("assets"), |_| false)?;
    }

    Ok(target_exe)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn check_dependencies() -> Result<()> {
    match std::env::consts::OS {
        "macos" => ensure_command("hdiutil"),
        "linux" => ensure_command("tar"),
        _ => Ok(()),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn check_dependencies() -> Result<()> {
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ensure_command(command: &str) -> Result<()> {
    Command::new(command)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
        .with_context(|| format!("could not find required command `{command}`"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ensure_output_success(output: Output, action: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(anyhow!(
        "{action} failed with status {}: {}{}{}",
        output.status,
        stderr.trim(),
        if stderr.trim().is_empty() || stdout.trim().is_empty() {
            ""
        } else {
            "\n"
        },
        stdout.trim()
    ))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn sync_dir_filtered(
    source: &Path,
    destination: &Path,
    should_skip: impl Fn(&Path) -> bool,
) -> Result<()> {
    if !source.is_dir() {
        bail!("source directory does not exist: {}", source.display());
    }

    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;

    delete_stale_entries(source, destination, &should_skip)?;
    copy_entries(source, destination, &should_skip)?;
    copy_permissions(source, destination)?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn delete_stale_entries(
    source: &Path,
    destination: &Path,
    should_skip: &impl Fn(&Path) -> bool,
) -> Result<()> {
    for entry in WalkDir::new(destination)
        .min_depth(1)
        .contents_first(true)
        .follow_links(false)
    {
        let entry = entry.with_context(|| format!("failed to read {}", destination.display()))?;
        let relative_path = entry
            .path()
            .strip_prefix(destination)
            .with_context(|| format!("failed to relativize {}", entry.path().display()))?;

        if should_skip(relative_path) {
            continue;
        }

        match fs::symlink_metadata(source.join(relative_path)) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                remove_path(entry.path()).with_context(|| {
                    format!("failed to remove stale {}", entry.path().display())
                })?;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect source entry {}",
                        source.join(relative_path).display()
                    )
                });
            }
        }
    }

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn copy_entries(
    source: &Path,
    destination: &Path,
    should_skip: &impl Fn(&Path) -> bool,
) -> Result<()> {
    let mut directories = Vec::new();

    for entry in WalkDir::new(source).min_depth(1).follow_links(false) {
        let entry = entry.with_context(|| format!("failed to read {}", source.display()))?;
        let relative_path = entry
            .path()
            .strip_prefix(source)
            .with_context(|| format!("failed to relativize {}", entry.path().display()))?;

        if should_skip(relative_path) {
            continue;
        }

        let destination_path = destination.join(relative_path);
        let file_type = entry.file_type();

        if file_type.is_dir() {
            prepare_destination_dir(&destination_path)?;
            directories.push((entry.path().to_path_buf(), destination_path));
        } else if file_type.is_symlink() {
            copy_symlink(entry.path(), &destination_path)?;
        } else if file_type.is_file() {
            copy_file(entry.path(), &destination_path)?;
        } else {
            bail!("unsupported update bundle entry {}", entry.path().display());
        }
    }

    for (source_dir, destination_dir) in directories.into_iter().rev() {
        copy_permissions(&source_dir, &destination_dir)?;
    }

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn prepare_destination_dir(path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_dir() {
            remove_path(path).with_context(|| {
                format!("failed to replace {} with a directory", path.display())
            })?;
        }
    }

    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    ensure_parent(destination)?;

    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if !metadata.is_file() {
            remove_path(destination)
                .with_context(|| format!("failed to replace {}", destination.display()))?;
        }
    }

    let temp_path = temp_path_for(destination)?;
    fs::copy(source, &temp_path).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    copy_permissions(source, &temp_path)?;

    if let Err(error) = fs::rename(&temp_path, destination) {
        let _ = fs::remove_file(&temp_path);
        return Err(error)
            .with_context(|| format!("failed to move {} into place", destination.display()));
    }

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn copy_symlink(source: &Path, destination: &Path) -> Result<()> {
    ensure_parent(destination)?;

    if fs::symlink_metadata(destination).is_ok() {
        remove_path(destination)
            .with_context(|| format!("failed to replace {}", destination.display()))?;
    }

    let target =
        fs::read_link(source).with_context(|| format!("failed to read {}", source.display()))?;
    std::os::unix::fs::symlink(&target, destination).with_context(|| {
        format!(
            "failed to create symlink {} -> {}",
            destination.display(),
            target.display()
        )
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn copy_permissions(source: &Path, destination: &Path) -> Result<()> {
    let permissions = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?
        .permissions();
    fs::set_permissions(destination, permissions)
        .with_context(|| format!("failed to set permissions on {}", destination.display()))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn remove_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    let file_type = metadata.file_type();

    if file_type.is_symlink() || file_type.is_file() {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))
    } else if file_type.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))
    } else {
        bail!("unsupported path type {}", path.display())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ensure_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn temp_path_for(destination: &Path) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", destination.display()))?;
    let file_name = destination
        .file_name()
        .ok_or_else(|| anyhow!("path has no file name: {}", destination.display()))?
        .to_string_lossy();

    for attempt in 0..1000 {
        let candidate = parent.join(format!(
            ".{file_name}.knotq-update-{}-{attempt}.tmp",
            std::process::id()
        ));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }

    bail!(
        "could not create temporary path next to {}",
        destination.display()
    )
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(target_os = "windows")]
fn powershell_string(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

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
