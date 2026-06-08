use std::fs::{self, File};
use std::io::{self, Read, Write};
#[cfg(target_os = "macos")]
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::time::{SystemTime, UNIX_EPOCH};

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/knotq-app/knotq-app/releases/latest";
const USER_AGENT: &str = concat!("KnotQ/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
pub struct AutoUpdateConfig {
    pub current_version: Version,
    pub latest_release_url: String,
    pub user_agent: String,
}

impl AutoUpdateConfig {
    pub fn github(current_version: Version) -> Self {
        Self {
            current_version,
            latest_release_url: LATEST_RELEASE_URL.into(),
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
    pub name: String,
    pub download_url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub sha256_url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallStrategy {
    InstalledOnRestart,
    OpenInstaller,
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

pub fn current_version(version: &str) -> Result<Version> {
    parse_version(version).with_context(|| format!("parse current app version {version:?}"))
}

pub fn check_latest_release(config: &AutoUpdateConfig) -> Result<LatestRelease> {
    let release = fetch_latest_release(config)?;
    let version = parse_version(&release.tag_name)
        .with_context(|| format!("parse latest release tag {:?}", release.tag_name))?;

    if version <= config.current_version {
        return Ok(LatestRelease::UpToDate {
            version,
            release_url: release.html_url,
        });
    }

    let asset = select_asset(&release.assets)?;
    Ok(LatestRelease::Available(AvailableUpdate {
        version,
        tag_name: release.tag_name,
        release_url: release.html_url,
        asset,
    }))
}

pub fn prepare_update(
    config: &AutoUpdateConfig,
    update: &AvailableUpdate,
    app_path: &Path,
    updates_dir: &Path,
) -> Result<StagedUpdate> {
    fs::create_dir_all(updates_dir)
        .with_context(|| format!("create updates directory {}", updates_dir.display()))?;
    let version_dir = updates_dir.join(update.version.to_string());
    if version_dir.exists() {
        fs::remove_dir_all(&version_dir)
            .with_context(|| format!("clear old updates directory {}", version_dir.display()))?;
    }
    fs::create_dir_all(&version_dir)
        .with_context(|| format!("create version updates directory {}", version_dir.display()))?;

    let asset_path = version_dir.join(&update.asset.name);
    download_asset(config, &update.asset, &asset_path)?;

    let kind = target_kind()?;
    let (install_strategy, restart_path) = match kind {
        TargetKind::MacOs => {
            prepare_macos_installer(&asset_path)?;
            (InstallStrategy::OpenInstaller, None)
        }
        TargetKind::Linux => {
            let restart_path = install_linux(&asset_path, app_path, updates_dir)?;
            (InstallStrategy::InstalledOnRestart, Some(restart_path))
        }
        TargetKind::Windows => (InstallStrategy::RunInstallerAndQuit, None),
    };

    Ok(StagedUpdate {
        version: update.version.clone(),
        tag_name: update.tag_name.clone(),
        release_url: update.release_url.clone(),
        asset_name: update.asset.name.clone(),
        asset_path,
        restart_path,
        install_strategy,
        staged_at: Utc::now(),
    })
}

#[cfg(target_os = "macos")]
fn prepare_macos_installer(downloaded_dmg: &Path) -> Result<()> {
    // The release app is App Sandbox signed. Child `hdiutil` processes inherit
    // that sandbox and cannot access disk-image devices, so preparation stops at
    // the verified DMG. The UI then opens it through Launch Services and lets the
    // user replace the app from Finder.
    validate_udif_disk_image(downloaded_dmg)
}

#[cfg(not(target_os = "macos"))]
fn prepare_macos_installer(_downloaded_dmg: &Path) -> Result<()> {
    bail!("macOS updater called on non-macOS platform")
}

pub fn open_staged_installer(update: &StagedUpdate) -> Result<()> {
    if update.install_strategy != InstallStrategy::OpenInstaller {
        bail!("staged update is not an installer that can be opened");
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&update.asset_path)
            .spawn()
            .with_context(|| format!("open update installer {}", update.asset_path.display()))?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        bail!("opening staged installers is not supported on this platform")
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
$install = Start-Process -FilePath $installer -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/update=true','/MERGETASKS=!desktopicon') -Wait -PassThru
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

fn fetch_latest_release(config: &AutoUpdateConfig) -> Result<GithubRelease> {
    let response = ureq::get(&config.latest_release_url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &config.user_agent)
        .call()
        .with_context(|| format!("fetch latest release from {}", config.latest_release_url))?;
    response
        .into_json()
        .context("decode latest GitHub release response")
}

fn download_asset(
    config: &AutoUpdateConfig,
    asset: &ReleaseAsset,
    destination: &Path,
) -> Result<()> {
    if let Ok(metadata) = fs::metadata(destination) {
        if asset.size.is_none_or(|size| metadata.len() == size) {
            return Ok(());
        }
    }

    let tmp_path = destination.with_extension("download");
    let response = ureq::get(&asset.download_url)
        .set("Accept", "application/octet-stream")
        .set("User-Agent", &config.user_agent)
        .call()
        .with_context(|| format!("download update asset {}", asset.name))?;

    let mut tmp_file = File::create(&tmp_path)
        .with_context(|| format!("create temporary download {}", tmp_path.display()))?;
    let copied = io::copy(&mut response.into_reader(), &mut tmp_file)
        .with_context(|| format!("write temporary download {}", tmp_path.display()))?;
    tmp_file
        .flush()
        .with_context(|| format!("flush temporary download {}", tmp_path.display()))?;
    if let Some(expected_size) = asset.size {
        if copied != expected_size {
            let _ = fs::remove_file(&tmp_path);
            bail!(
                "downloaded {} bytes for {}, expected {}",
                copied,
                asset.name,
                expected_size
            );
        }
    }
    fs::rename(&tmp_path, destination)
        .with_context(|| format!("move download into {}", destination.display()))?;
    verify_asset(config, asset, destination)?;
    Ok(())
}

fn parse_version(raw: &str) -> Result<Version> {
    let trimmed = raw.trim().trim_start_matches('v');
    Version::parse(trimmed).map_err(Into::into)
}

fn target_kind() -> Result<TargetKind> {
    match std::env::consts::OS {
        "macos" => Ok(TargetKind::MacOs),
        "linux" => Ok(TargetKind::Linux),
        "windows" => Ok(TargetKind::Windows),
        os => bail!("auto updates are not supported on {os}"),
    }
}

fn select_asset(assets: &[GithubAsset]) -> Result<ReleaseAsset> {
    let profile = target_asset_profile()?;
    let asset = assets
        .iter()
        .filter(|asset| asset.state.as_deref().unwrap_or("uploaded") == "uploaded")
        .filter(|asset| profile.matches(&asset.name))
        .max_by_key(|asset| profile.score(&asset.name))
        .ok_or_else(|| anyhow!("no update asset matched {}", profile.description))?;
    let checksum_asset = checksum_asset_for(assets, &asset.name);
    Ok(ReleaseAsset {
        name: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
        size: asset.size,
        sha256: github_asset_sha256(asset),
        sha256_url: checksum_asset.map(|asset| asset.browser_download_url.clone()),
    })
}

fn target_asset_profile() -> Result<AssetProfile> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok(AssetProfile::new(
            "macOS arm64 DMG",
            &["macos", "arm64"],
            &[".dmg"],
            &[],
        )),
        ("macos", "x86_64") => Ok(AssetProfile::new(
            "macOS x86_64 DMG",
            &["macos", "x86_64"],
            &[".dmg"],
            &[],
        )),
        ("linux", "x86_64") => Ok(AssetProfile::new(
            "Linux x86_64 tarball",
            &["linux", "x86_64"],
            &[".tar.gz"],
            &[],
        )),
        ("windows", "x86_64") => Ok(AssetProfile::new(
            "Windows x64 setup executable",
            &["windows", "x64", "setup"],
            &[".exe"],
            &["setup"],
        )),
        (os, arch) => bail!("auto updates are not supported on {os}/{arch}"),
    }
}

fn checksum_asset_for<'a>(assets: &'a [GithubAsset], asset_name: &str) -> Option<&'a GithubAsset> {
    let candidates = [
        format!("{asset_name}.sha256"),
        format!("{asset_name}.sha256sum"),
    ];
    assets.iter().find(|asset| {
        asset.state.as_deref().unwrap_or("uploaded") == "uploaded"
            && candidates.iter().any(|candidate| asset.name == *candidate)
    })
}

fn github_asset_sha256(asset: &GithubAsset) -> Option<String> {
    asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .map(str::trim)
        .filter(|digest| is_sha256_hex(digest))
        .map(str::to_ascii_lowercase)
}

fn verify_asset(config: &AutoUpdateConfig, asset: &ReleaseAsset, destination: &Path) -> Result<()> {
    let Some(expected) = expected_sha256(config, asset)? else {
        return Ok(());
    };
    let actual = sha256_file(destination)?;
    if actual != expected {
        bail!(
            "downloaded update checksum mismatch for {}: expected {}, got {}",
            asset.name,
            expected,
            actual
        );
    }
    Ok(())
}

fn expected_sha256(config: &AutoUpdateConfig, asset: &ReleaseAsset) -> Result<Option<String>> {
    if let Some(sha256) = asset
        .sha256
        .as_deref()
        .map(str::trim)
        .filter(|sha256| is_sha256_hex(sha256))
    {
        return Ok(Some(sha256.to_ascii_lowercase()));
    }

    let Some(url) = &asset.sha256_url else {
        return Ok(None);
    };
    let response = ureq::get(url)
        .set("Accept", "text/plain")
        .set("User-Agent", &config.user_agent)
        .call()
        .with_context(|| format!("download checksum for {}", asset.name))?;
    let text = response
        .into_string()
        .with_context(|| format!("read checksum for {}", asset.name))?;
    Ok(Some(parse_sha256_checksum(&text).with_context(|| {
        format!("parse checksum for {}", asset.name)
    })?))
}

fn parse_sha256_checksum(raw: &str) -> Result<String> {
    for token in raw.split_whitespace() {
        let token = token.trim().trim_start_matches("sha256:");
        if is_sha256_hex(token) {
            return Ok(token.to_ascii_lowercase());
        }
    }
    bail!("checksum file did not contain a SHA-256 digest")
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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

#[cfg(target_os = "macos")]
fn validate_udif_disk_image(path: &Path) -> Result<()> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let len = file
        .metadata()
        .with_context(|| format!("inspect {}", path.display()))?
        .len();
    if len < 512 {
        bail!("downloaded macOS update is too small to be a DMG");
    }

    let mut trailer = [0_u8; 512];
    file.seek(SeekFrom::End(-512))
        .with_context(|| format!("seek DMG trailer in {}", path.display()))?;
    file.read_exact(&mut trailer)
        .with_context(|| format!("read DMG trailer from {}", path.display()))?;
    if &trailer[..4] != b"koly" {
        bail!("downloaded macOS update is not a UDIF disk image");
    }
    Ok(())
}

struct AssetProfile {
    description: &'static str,
    required: &'static [&'static str],
    suffixes: &'static [&'static str],
    preferred: &'static [&'static str],
}

impl AssetProfile {
    fn new(
        description: &'static str,
        required: &'static [&'static str],
        suffixes: &'static [&'static str],
        preferred: &'static [&'static str],
    ) -> Self {
        Self {
            description,
            required,
            suffixes,
            preferred,
        }
    }

    fn matches(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.required
            .iter()
            .all(|fragment| lower.contains(fragment))
            && self.suffixes.iter().any(|suffix| lower.ends_with(suffix))
    }

    fn score(&self, name: &str) -> usize {
        let lower = name.to_ascii_lowercase();
        self.preferred
            .iter()
            .filter(|fragment| lower.contains(**fragment))
            .count()
    }
}

#[cfg(target_os = "linux")]
fn install_linux(
    downloaded_tar_gz: &Path,
    running_exe_path: &Path,
    updates_dir: &Path,
) -> Result<PathBuf> {
    let install_dir = running_exe_path
        .parent()
        .ok_or_else(|| anyhow!("running executable has no parent directory"))?;
    let extract_dir = unique_temp_dir(updates_dir, "extract")?;
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(downloaded_tar_gz)
        .arg("-C")
        .arg(&extract_dir)
        .output()
        .context("extract Linux update")?;
    if !output.status.success() {
        bail!(
            "failed to extract Linux update: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let source_exe = extract_dir.join("knotq");
    let target_exe = install_dir.join("knotq");
    let temp_exe = install_dir.join(format!(
        ".knotq-update-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::copy(&source_exe, &temp_exe).with_context(|| {
        format!(
            "copy update binary from {} to {}",
            source_exe.display(),
            temp_exe.display()
        )
    })?;
    fs::set_permissions(&temp_exe, fs::metadata(&source_exe)?.permissions())
        .with_context(|| format!("set permissions on {}", temp_exe.display()))?;
    fs::rename(&temp_exe, &target_exe)
        .with_context(|| format!("replace executable {}", target_exe.display()))?;

    let source_assets = extract_dir.join("assets");
    if source_assets.is_dir() {
        copy_dir_contents(&source_assets, &install_dir.join("assets"))?;
    }
    let _ = fs::remove_dir_all(&extract_dir);
    Ok(install_dir.join("knotq"))
}

#[cfg(target_os = "linux")]
fn copy_dir_contents(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("create {}", target.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_contents(&source_path, &target_path)?;
        } else if file_type.is_symlink() {
            let link_target = fs::read_link(&source_path)
                .with_context(|| format!("read symlink {}", source_path.display()))?;
            let _ = fs::remove_file(&target_path);
            std::os::unix::fs::symlink(&link_target, &target_path).with_context(|| {
                format!(
                    "copy symlink {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        } else {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "copy asset {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
            fs::set_permissions(&target_path, fs::metadata(&source_path)?.permissions())
                .with_context(|| format!("set permissions on {}", target_path.display()))?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn install_linux(
    _downloaded_tar_gz: &Path,
    _running_exe_path: &Path,
    _updates_dir: &Path,
) -> Result<PathBuf> {
    bail!("Linux updater called on non-Linux platform")
}

#[cfg(target_os = "windows")]
fn powershell_string(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

#[cfg(target_os = "linux")]
fn unique_temp_dir(parent: &Path, prefix: &str) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = parent.join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path)
        .with_context(|| format!("create temp directory {}", path.display()))?;
    Ok(path)
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    digest: Option<String>,
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
    fn older_release_is_up_to_date() {
        let config = AutoUpdateConfig {
            current_version: Version::new(1, 2, 3),
            latest_release_url: "https://example.invalid".into(),
            user_agent: USER_AGENT.into(),
        };
        let release = GithubRelease {
            tag_name: "v1.2.3".into(),
            html_url: "https://example.test/releases/v1.2.3".into(),
            assets: Vec::new(),
        };
        let version = parse_version(&release.tag_name).unwrap();
        assert!(version <= config.current_version);
    }

    #[test]
    fn asset_profile_matches_expected_suffix_and_fragments() {
        let profile = AssetProfile::new(
            "Linux x86_64 tarball",
            &["linux", "x86_64"],
            &[".tar.gz"],
            &[],
        );
        assert!(profile.matches("KnotQ-1.2.3-linux-x86_64.tar.gz"));
        assert!(!profile.matches("KnotQ-1.2.3-linux-x86_64.zip"));
        assert!(!profile.matches("KnotQ-1.2.3-macos-x86_64.dmg"));
    }

    #[test]
    fn parses_checksum_sidecar_contents() {
        let checksum = "A".repeat(64);
        let parsed = parse_sha256_checksum(&format!("{checksum}  KnotQ.dmg\n")).unwrap();
        assert_eq!(parsed, checksum.to_ascii_lowercase());
    }

    #[test]
    fn selects_checksum_sidecar_for_asset() {
        let assets = vec![
            GithubAsset {
                name: "KnotQ-1.2.3-macos-arm64.dmg".into(),
                browser_download_url: "https://example.test/KnotQ.dmg".into(),
                size: Some(10),
                state: Some("uploaded".into()),
                digest: None,
            },
            GithubAsset {
                name: "KnotQ-1.2.3-macos-arm64.dmg.sha256".into(),
                browser_download_url: "https://example.test/KnotQ.dmg.sha256".into(),
                size: Some(80),
                state: Some("uploaded".into()),
                digest: None,
            },
        ];

        let checksum = checksum_asset_for(&assets, "KnotQ-1.2.3-macos-arm64.dmg").unwrap();
        assert_eq!(
            checksum.browser_download_url,
            "https://example.test/KnotQ.dmg.sha256"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn validates_udif_trailer_before_opening_macos_installer() {
        let path = std::env::temp_dir().join(format!(
            "knotq-update-test-{}-{}.dmg",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let mut bytes = vec![0_u8; 512];
        bytes[..4].copy_from_slice(b"koly");
        fs::write(&path, &bytes).unwrap();

        let result = validate_udif_disk_image(&path);
        let _ = fs::remove_file(&path);

        assert!(result.is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_non_udif_macos_installer_downloads() {
        let path = std::env::temp_dir().join(format!(
            "knotq-update-test-{}-{}.dmg",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::write(&path, vec![0_u8; 512]).unwrap();

        let result = validate_udif_disk_image(&path);
        let _ = fs::remove_file(&path);

        assert!(result.is_err());
    }
}
