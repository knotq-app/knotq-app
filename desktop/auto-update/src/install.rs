#[cfg(target_os = "macos")]
use std::ffi::OsStr;
use std::fs;
#[cfg(target_os = "macos")]
use std::io;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command as PlatformCommand;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Output, Stdio};

use anyhow::{anyhow, bail, Context, Result};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use walkdir::WalkDir;

/// The name of the directory inside a version directory that holds the unpacked
/// payload: the new `.app` bundle on macOS, the extracted tree on Linux.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) const PAYLOAD_DIR: &str = "payload";

/// Where a disk image is attached while its bundle is copied out of it.
#[cfg(target_os = "macos")]
const MOUNT_DIR: &str = "mount";

/// `hdiutil` refuses to attach while another attach is in flight ("Resource
/// temporarily unavailable"), so the tests that build and mount disk images take
/// turns. A real update only ever runs one of these at a time.
#[cfg(test)]
pub(crate) fn disk_image_tool_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Unpack a verified download into `version_dir/payload` so that installing it
/// later is nothing but a local file copy.
///
/// `Ok(None)` means this platform installs the downloaded file as it is (the
/// Windows installer), and the archive must be kept.
#[cfg(target_os = "macos")]
pub(crate) fn stage_payload(
    downloaded_asset: &Path,
    version_dir: &Path,
    app_path: &Path,
) -> Result<Option<PathBuf>> {
    let app_name = app_path
        .file_name()
        .ok_or_else(|| anyhow!("invalid app bundle path {}", app_path.display()))?
        .to_os_string();
    stage_macos_payload(downloaded_asset, version_dir, &app_name).map(Some)
}

#[cfg(target_os = "linux")]
pub(crate) fn stage_payload(
    downloaded_asset: &Path,
    version_dir: &Path,
    _app_path: &Path,
) -> Result<Option<PathBuf>> {
    stage_linux_payload(downloaded_asset, version_dir).map(Some)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn stage_payload(
    _downloaded_asset: &Path,
    _version_dir: &Path,
    _app_path: &Path,
) -> Result<Option<PathBuf>> {
    Ok(None)
}

/// Install an update that is already on disk. `payload` is the directory
/// [`stage_payload`] produced; the archive path is only used by a staged update
/// that predates payload staging.
#[cfg(target_os = "macos")]
pub(crate) fn install_restart_update(
    payload: Option<&Path>,
    downloaded_asset: &Path,
    download_dir: &Path,
) -> Result<Option<PathBuf>> {
    let app_path = current_macos_app_bundle()?;
    match payload {
        Some(staged_app) => install_macos_from_payload(staged_app, &app_path)?,
        None => install_macos_update(downloaded_asset, download_dir, &app_path)?,
    }
    Ok(None)
}

#[cfg(target_os = "linux")]
pub(crate) fn install_restart_update(
    payload: Option<&Path>,
    downloaded_asset: &Path,
    download_dir: &Path,
) -> Result<Option<PathBuf>> {
    match payload {
        Some(extracted) => install_linux_from_payload(extracted).map(Some),
        None => install_linux_update(downloaded_asset, download_dir).map(Some),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn install_restart_update(
    _payload: Option<&Path>,
    _downloaded_asset: &Path,
    _download_dir: &Path,
) -> Result<Option<PathBuf>> {
    bail!(
        "restart updates are not supported on {}",
        std::env::consts::OS
    )
}

/// Mount the disk image, copy the new bundle out of it, and unmount again.
///
/// This is the slow half of a macOS update — `hdiutil attach` plus reading every
/// byte of the bundle back through a compressed image — and none of it needs the
/// app to be shutting down, so it happens while the update is being staged
/// rather than after the user asks to restart.
#[cfg(target_os = "macos")]
fn stage_macos_payload(
    downloaded_dmg: &Path,
    version_dir: &Path,
    app_name: &OsStr,
) -> Result<PathBuf> {
    let payload_root = version_dir.join(PAYLOAD_DIR);
    if payload_root.exists() {
        fs::remove_dir_all(&payload_root)
            .with_context(|| format!("failed to clear {}", payload_root.display()))?;
    }
    fs::create_dir_all(&payload_root)
        .with_context(|| format!("failed to create {}", payload_root.display()))?;
    let staged_app = payload_root.join(app_name);

    let mount_root = version_dir.join(MOUNT_DIR);
    release_abandoned_mounts(version_dir);
    let _ = fs::remove_dir_all(&mount_root);
    fs::create_dir_all(&mount_root)
        .with_context(|| format!("failed to create {}", mount_root.display()))?;

    let output = PlatformCommand::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(downloaded_dmg)
        .arg("-mountroot")
        .arg(&mount_root)
        .output()
        .context("failed to mount update disk image")?;
    ensure_output_success(output, "mount update disk image")?;

    let copy_result =
        find_mounted_app(&mount_root, app_name).and_then(|(mount_path, mounted_app)| {
            let copied = sync_dir_filtered(&mounted_app, &staged_app, macos_finder_icon_file);
            let detached = detach_disk_image(&mount_path);
            copied.and(detached)
        });

    // An abandoned mount outlives the process and keeps the image busy, so the
    // mount root is swept whether or not the copy worked.
    let _ = detach_every_mount(&mount_root);
    let _ = fs::remove_dir_all(&mount_root);

    copy_result.map(|()| staged_app)
}

#[cfg(target_os = "macos")]
fn install_macos_from_payload(staged_app: &Path, app_path: &Path) -> Result<()> {
    if !staged_app.is_dir() {
        bail!("staged update bundle is missing: {}", staged_app.display());
    }
    sync_dir_filtered(staged_app, app_path, macos_finder_icon_file)
}

#[cfg(target_os = "macos")]
fn detach_disk_image(mount_path: &Path) -> Result<()> {
    PlatformCommand::new("hdiutil")
        .args(["detach", "-force"])
        .arg(mount_path)
        .output()
        .context("failed to detach update disk image")
        .and_then(|output| ensure_output_success(output, "detach update disk image"))
}

/// Detach whatever an interrupted staging left attached under `version_dir`.
///
/// A mount outlives the process that made it, and the image is attached
/// read-only, so a version directory with one still in it cannot be removed:
/// the delete walks into the volume and fails. Staging runs in the background,
/// where quitting part-way through is ordinary, so every path that clears a
/// version directory releases its mounts first.
#[cfg(target_os = "macos")]
pub(crate) fn release_abandoned_mounts(version_dir: &Path) {
    let mount_root = version_dir.join(MOUNT_DIR);
    if mount_root.is_dir() {
        let _ = detach_every_mount(&mount_root);
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn release_abandoned_mounts(_version_dir: &Path) {}

#[cfg(target_os = "macos")]
fn detach_every_mount(mount_root: &Path) -> Result<()> {
    for entry in fs::read_dir(mount_root)
        .with_context(|| format!("failed to read {}", mount_root.display()))?
    {
        let path = entry?.path();
        if path.is_dir() {
            let _ = detach_disk_image(&path);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_macos_update(downloaded_dmg: &Path, temp_dir: &Path, app_path: &Path) -> Result<()> {
    let app_name = app_path
        .file_name()
        .ok_or_else(|| anyhow!("invalid app bundle path {}", app_path.display()))?
        .to_os_string();
    let staged_app = stage_macos_payload(downloaded_dmg, temp_dir, &app_name)?;
    install_macos_from_payload(&staged_app, app_path)
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

/// Extract the release tarball into `version_dir/payload`, and check it holds
/// what the install step will look for. Decompression is the slow half of a
/// Linux update, so it happens here rather than after the user asks to restart.
#[cfg(target_os = "linux")]
fn stage_linux_payload(downloaded_tar_gz: &Path, version_dir: &Path) -> Result<PathBuf> {
    let extracted = version_dir.join(PAYLOAD_DIR);
    if extracted.exists() {
        fs::remove_dir_all(&extracted)
            .with_context(|| format!("failed to clear {}", extracted.display()))?;
    }
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

    if !extracted.join("knotq").is_file() {
        bail!("Linux update did not contain knotq");
    }
    if !extracted.join("knotq-mcp").is_file() {
        bail!("Linux update did not contain knotq-mcp");
    }

    Ok(extracted)
}

#[cfg(target_os = "linux")]
fn install_linux_update(downloaded_tar_gz: &Path, temp_dir: &Path) -> Result<PathBuf> {
    let extracted = stage_linux_payload(downloaded_tar_gz, temp_dir)?;
    install_linux_from_payload(&extracted)
}

#[cfg(target_os = "linux")]
fn install_linux_from_payload(extracted: &Path) -> Result<PathBuf> {
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

    let source_mcp_bridge = extracted.join("knotq-mcp");
    if !source_mcp_bridge.is_file() {
        bail!("Linux update did not contain knotq-mcp");
    }
    copy_file(&source_mcp_bridge, &install_dir.join("knotq-mcp"))?;

    let source_assets = extracted.join("assets");
    if source_assets.is_dir() {
        install_linux_assets(&source_assets, &install_dir.join("assets"))?;
    }

    Ok(target_exe)
}

#[cfg(target_os = "linux")]
fn install_linux_assets(source: &Path, destination: &Path) -> Result<()> {
    // Older installers placed `assets` beside ~/.local/bin/knotq. That generic
    // directory may be shared by unrelated software, so Linux updates must never
    // prune entries that are absent from KnotQ's archive.
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    copy_entries(source, destination, &|_| false)?;
    copy_permissions(source, destination)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn check_dependencies() -> Result<()> {
    match std::env::consts::OS {
        "macos" => ensure_command("hdiutil"),
        "linux" => ensure_command("tar"),
        _ => Ok(()),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn check_dependencies() -> Result<()> {
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "windows")]
pub(crate) fn powershell_string(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    #[test]
    fn linux_asset_update_preserves_unrelated_destination_files() {
        let root = std::env::temp_dir().join(format!(
            "knotq-linux-assets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("knotq.svg"), b"new").unwrap();
        fs::write(destination.join("unrelated.txt"), b"keep").unwrap();

        install_linux_assets(&source, &destination).unwrap();

        assert_eq!(fs::read(destination.join("knotq.svg")).unwrap(), b"new");
        assert_eq!(
            fs::read(destination.join("unrelated.txt")).unwrap(),
            b"keep"
        );
        fs::remove_dir_all(root).ok();
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "knotq-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An app bundle shaped like the real one: a nested executable, a symlink of
    /// the kind every framework bundle contains, and a resource.
    fn write_app_bundle(app: &Path, marker: &str) {
        let macos = app.join("Contents/MacOS");
        let resources = app.join("Contents/Resources");
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(&resources).unwrap();
        fs::write(app.join("Contents/Info.plist"), marker.as_bytes()).unwrap();
        fs::write(macos.join("KnotQ"), format!("binary {marker}")).unwrap();
        fs::set_permissions(macos.join("KnotQ"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(resources.join("icon.icns"), format!("icon {marker}")).unwrap();
        std::os::unix::fs::symlink("Resources", app.join("Contents/Current")).unwrap();
    }

    /// Build a disk image laid out the way `bundling/macos/post-bundle.sh` does:
    /// the bundle plus an `/Applications` symlink.
    fn build_dmg(dir: &Path, app_name: &str, marker: &str) -> PathBuf {
        let stage = dir.join("stage");
        fs::create_dir_all(&stage).unwrap();
        write_app_bundle(&stage.join(app_name), marker);
        std::os::unix::fs::symlink("/Applications", stage.join("Applications")).unwrap();

        let dmg = dir.join("KnotQ.dmg");
        let output = PlatformCommand::new("hdiutil")
            .args(["create", "-volname", "KnotQ", "-srcfolder"])
            .arg(&stage)
            .args(["-ov", "-format", "UDZO", "-quiet"])
            .arg(&dmg)
            .output()
            .expect("hdiutil create");
        assert!(
            output.status.success(),
            "hdiutil create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        dmg
    }

    /// The whole point of staging: mounting the image and reading the bundle out
    /// of it happens while nothing waits on it, so what is left on disk is a
    /// plain directory the install step can copy.
    #[test]
    fn staging_unpacks_the_bundle_out_of_the_disk_image() {
        let _serialized = disk_image_tool_lock();
        let root = scratch_dir("stage-dmg");
        let dmg = build_dmg(&root, "KnotQ.app", "new");
        let version_dir = root.join("0.60.0");
        fs::create_dir_all(&version_dir).unwrap();

        let staged = stage_macos_payload(&dmg, &version_dir, OsStr::new("KnotQ.app")).unwrap();

        assert_eq!(staged, version_dir.join(PAYLOAD_DIR).join("KnotQ.app"));
        assert_eq!(
            fs::read_to_string(staged.join("Contents/Info.plist")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(staged.join("Contents/MacOS/KnotQ")).unwrap(),
            "binary new"
        );
        assert_eq!(
            fs::read_link(staged.join("Contents/Current")).unwrap(),
            Path::new("Resources")
        );
        assert_eq!(
            fs::metadata(staged.join("Contents/MacOS/KnotQ"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        // Nothing may be left mounted: a live mount holds the image open and
        // would survive the process that made it.
        assert!(!version_dir.join("mount").exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// Installing from a staged payload has to produce exactly the new bundle:
    /// files replaced, files the release dropped removed, symlinks and modes
    /// intact. No disk image: by this point staging has already unpacked one.
    #[test]
    fn installing_from_a_staged_payload_replaces_the_installed_bundle() {
        let root = scratch_dir("install-payload");
        let staged = root.join(PAYLOAD_DIR).join("KnotQ.app");
        write_app_bundle(&staged, "new");

        let installed = root.join("Applications").join("KnotQ.app");
        write_app_bundle(&installed, "old");
        let dropped = installed.join("Contents/Resources/removed-in-0.60.0.txt");
        fs::write(&dropped, b"from the previous release").unwrap();
        // A directory the new release no longer has at all.
        let dropped_dir = installed.join("Contents/Frameworks/Old.framework");
        fs::create_dir_all(&dropped_dir).unwrap();
        fs::write(dropped_dir.join("Old"), b"gone in the new release").unwrap();

        install_macos_from_payload(&staged, &installed).unwrap();

        assert_eq!(
            fs::read_to_string(installed.join("Contents/Info.plist")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(installed.join("Contents/MacOS/KnotQ")).unwrap(),
            "binary new"
        );
        assert!(!dropped.exists(), "a file the new release dropped was kept");
        assert!(
            !dropped_dir.exists(),
            "a directory the new release dropped was kept"
        );
        assert_eq!(
            fs::read_link(installed.join("Contents/Current")).unwrap(),
            Path::new("Resources")
        );
        assert_eq!(
            fs::metadata(installed.join("Contents/MacOS/KnotQ"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Installing must refuse rather than half-copy when the payload it was told
    /// about is not there.
    #[test]
    fn installing_without_a_payload_fails_before_touching_the_bundle() {
        let root = scratch_dir("install-missing-payload");
        let installed = root.join("Applications").join("KnotQ.app");
        write_app_bundle(&installed, "old");

        let error =
            install_macos_from_payload(&root.join("payload/KnotQ.app"), &installed).unwrap_err();

        assert!(
            format!("{error:#}").contains("staged update bundle is missing"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            fs::read_to_string(installed.join("Contents/Info.plist")).unwrap(),
            "old",
            "the installed bundle was modified by a failed install"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Staging over an existing payload — a retried download, a re-released tag —
    /// must leave the payload as the new image, not a merge of both.
    #[test]
    fn staging_again_over_an_existing_payload_starts_clean() {
        let _serialized = disk_image_tool_lock();
        let root = scratch_dir("stage-again");
        let version_dir = root.join("0.60.0");
        fs::create_dir_all(&version_dir).unwrap();

        // What an earlier, different staging left behind.
        let previous = version_dir.join(PAYLOAD_DIR).join("KnotQ.app");
        write_app_bundle(&previous, "first");
        let only_in_first = previous.join("Contents/Resources/only-in-first.txt");
        fs::write(&only_in_first, b"stale").unwrap();

        let dmg = build_dmg(&root, "KnotQ.app", "second");
        let staged = stage_macos_payload(&dmg, &version_dir, OsStr::new("KnotQ.app")).unwrap();

        assert_eq!(staged, previous);
        assert_eq!(
            fs::read_to_string(staged.join("Contents/Info.plist")).unwrap(),
            "second"
        );
        assert!(
            !only_in_first.exists(),
            "the previous payload leaked through"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A disk image that does not hold the bundle we are updating must fail, and
    /// must not leave the image mounted behind it.
    #[test]
    fn a_disk_image_without_the_bundle_fails_without_leaving_a_mount() {
        let _serialized = disk_image_tool_lock();
        let root = scratch_dir("stage-wrong-dmg");
        let dmg = build_dmg(&root, "SomethingElse.app", "new");
        let version_dir = root.join("0.60.0");
        fs::create_dir_all(&version_dir).unwrap();

        let error = stage_macos_payload(&dmg, &version_dir, OsStr::new("KnotQ.app")).unwrap_err();

        assert!(
            format!("{error:#}").contains("KnotQ.app"),
            "unexpected error: {error:#}"
        );
        assert!(!version_dir.join("mount").exists());

        let _ = fs::remove_dir_all(&root);
    }
}
