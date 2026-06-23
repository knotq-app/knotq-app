use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::{AutoUpdateConfig, ReleaseAsset};

pub(crate) fn download_asset(
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

pub(crate) fn verify_asset(path: &Path, expected_sha256: &str) -> Result<()> {
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

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
