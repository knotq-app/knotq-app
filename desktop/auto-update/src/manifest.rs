use anyhow::{bail, Context, Result};
use semver::{BuildMetadata, Prerelease, Version};

use crate::{
    AutoUpdateConfig, ManifestAsset, TargetKind, UpdateManifest, DEFAULT_MANIFEST_URL,
};

pub(crate) fn fetch_manifest(config: &AutoUpdateConfig) -> Result<UpdateManifest> {
    let response = ureq::get(&config.update_manifest_url)
        .set("Accept", "application/json")
        .set("User-Agent", &config.user_agent)
        .call()
        .with_context(|| format!("fetch update manifest from {}", config.update_manifest_url))?;
    response.into_json().context("decode update manifest JSON")
}

pub(crate) fn parse_version(raw: &str) -> Result<Version> {
    let trimmed = raw.trim().trim_start_matches('v');
    Version::parse(trimmed).map_err(Into::into)
}

pub(crate) fn version_is_newer(current_version: &Version, fetched_version: &Version) -> bool {
    normalize_version(fetched_version) > normalize_version(current_version)
}

fn normalize_version(version: &Version) -> Version {
    let mut normalized = version.clone();
    normalized.pre = Prerelease::EMPTY;
    normalized.build = BuildMetadata::EMPTY;
    normalized
}

pub(crate) fn update_manifest_url() -> String {
    std::env::var("KNOTQ_UPDATE_MANIFEST_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .or_else(|| option_env!("KNOTQ_UPDATE_MANIFEST_URL").map(str::to_string))
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_string())
}

pub(crate) fn matching_asset(manifest: &UpdateManifest) -> Option<&ManifestAsset> {
    matching_asset_for(manifest, std::env::consts::OS, std::env::consts::ARCH)
}

pub(crate) fn matching_asset_for<'a>(
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

pub(crate) fn validate_asset_kind(asset: &ManifestAsset) -> Result<()> {
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

pub(crate) fn target_kind() -> Result<TargetKind> {
    match std::env::consts::OS {
        "macos" => Ok(TargetKind::MacOs),
        "linux" => Ok(TargetKind::Linux),
        "windows" => Ok(TargetKind::Windows),
        os => bail!("auto updates are not supported on {os}"),
    }
}
