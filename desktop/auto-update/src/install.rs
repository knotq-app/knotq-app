use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Output, Stdio};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{ffi::OsStr, process::Command as PlatformCommand};

use anyhow::{anyhow, bail, Context, Result};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use walkdir::WalkDir;

#[cfg(target_os = "macos")]
pub(crate) fn install_restart_update(
    downloaded_asset: &Path,
    download_dir: &Path,
) -> Result<Option<PathBuf>> {
    install_macos_update(downloaded_asset, download_dir)?;
    Ok(None)
}

#[cfg(target_os = "linux")]
pub(crate) fn install_restart_update(
    downloaded_asset: &Path,
    download_dir: &Path,
) -> Result<Option<PathBuf>> {
    install_linux_update(downloaded_asset, download_dir).map(Some)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn install_restart_update(
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

#[cfg(target_os = "windows")]
pub(crate) fn powershell_string(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}
