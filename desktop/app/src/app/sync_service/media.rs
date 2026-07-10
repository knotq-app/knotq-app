use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use knotq_model::{DocumentId, ImageAssetFormat, ImageInline, Item, ItemContent, Workspace};
use knotq_storage_json::image_asset_path;
use knotq_sync::{LocalSyncState, MAX_SYNC_MEDIA_BYTES};
use sha2::{Digest, Sha256};

use super::{SyncHttpClient, SyncMediaAsset};

impl SyncMediaAsset {
    pub(super) fn image_name(self) -> String {
        format!("{}.{}", self.asset, self.format.extension())
    }
}

fn workspace_media_assets(workspace: &Workspace) -> Vec<SyncMediaAsset> {
    let mut seen = HashSet::new();
    let mut assets = Vec::new();
    for scheme in workspace.iter_schemes() {
        let Some(meta) = workspace.scheme_sync.get(&scheme.id) else {
            continue;
        };
        for item in &scheme.items {
            for image in item_image_assets(item) {
                let media = SyncMediaAsset {
                    document: meta.id,
                    asset: image.asset,
                    format: image.format,
                };
                if seen.insert(media) {
                    assets.push(media);
                }
            }
        }
    }
    assets
}

fn item_image_assets(item: &Item) -> Vec<ImageInline> {
    let mut images = Vec::new();
    collect_item_image_assets(item, &mut images);
    images
}

fn collect_item_image_assets(item: &Item, images: &mut Vec<ImageInline>) {
    match &item.content {
        ItemContent::Text { .. } => {}
        ItemContent::Image(image) => images.push(*image),
        ItemContent::Table(table) => {
            for cell in table.cells() {
                for item in &cell.items {
                    collect_item_image_assets(item, images);
                }
            }
        }
    }
}

pub(super) fn upload_local_media_assets(
    client: &SyncHttpClient,
    local_state: &mut LocalSyncState,
    workspace: &Workspace,
    remote_latest: &HashMap<DocumentId, u64>,
) -> Result<()> {
    for media in workspace_media_assets(workspace) {
        let path = image_asset_path(media.asset, media.format.extension());
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let byte_length = metadata.len();
        if byte_length == 0 {
            continue;
        }
        if byte_length > MAX_SYNC_MEDIA_BYTES as u64 {
            // An over-limit asset can never upload; skipping it (rather than
            // returning Err) keeps a single bad image from permanently wedging the
            // CRDT sync, so text/structure edits still converge.
            eprintln!(
                "sync: skipping image {} ({} bytes, above the {} byte sync limit)",
                media.image_name(),
                byte_length,
                MAX_SYNC_MEDIA_BYTES
            );
            continue;
        }
        let image_name = media.image_name();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("sync: skipping unreadable image {image_name}: {error}");
                continue;
            }
        };
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            eprintln!(
                "sync: skipping image {} ({} bytes, above the {} byte sync limit)",
                image_name,
                bytes.len(),
                MAX_SYNC_MEDIA_BYTES
            );
            continue;
        }
        let sha256 = media_sha256(&bytes);
        if !local_state.should_upload_media_asset(
            &image_name,
            media.document,
            byte_length,
            &sha256,
            remote_latest,
        ) {
            continue;
        }
        // A single asset's upload failure (server rejection, transient error) must
        // not abort the whole sync before CRDT pull cursors are persisted — skip it
        // and let the next sync retry, rather than wedging every future sync.
        if let Err(error) = client.upload_media_asset(media, &bytes) {
            eprintln!("sync: media upload failed for {image_name}; skipping: {error:#}");
            continue;
        }
        local_state.mark_media_uploaded(image_name, media.document, byte_length, sha256);
    }
    Ok(())
}

pub(super) fn download_missing_media_assets(
    client: &SyncHttpClient,
    workspace: &Workspace,
) -> Result<bool> {
    let mut downloaded = false;
    for media in workspace_media_assets(workspace) {
        let path = image_asset_path(media.asset, media.format.extension());
        match media_asset_needs_download(&path) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) => {
                eprintln!(
                    "sync: skipping media download for {}: {error}",
                    media.image_name()
                );
                continue;
            }
        }
        let image_name = media.image_name();
        // A single asset's download failure must not abort the whole sync — skip it
        // and let a later sync retry, mirroring the missing-asset skip below.
        let bytes = match client.download_media_asset(media) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                eprintln!("sync media missing on backend: {image_name}; skipping download");
                continue;
            }
            Err(error) => {
                eprintln!("sync: media download failed for {image_name}; skipping: {error:#}");
                continue;
            }
        };
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            eprintln!(
                "sync: skipping oversized downloaded image {} ({} bytes, above the {} byte limit)",
                image_name,
                bytes.len(),
                MAX_SYNC_MEDIA_BYTES
            );
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        if let Err(error) = fs::write(&path, bytes) {
            eprintln!("sync: failed to write downloaded image {image_name}; skipping: {error}");
            continue;
        }
        downloaded = true;
    }
    Ok(downloaded)
}

pub(super) fn media_asset_needs_download(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => Ok(false),
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(anyhow!(
            "image asset path {} exists but is not a file",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).with_context(|| format!("stat {}", path.display())),
    }
}

pub(super) fn media_content_type(format: ImageAssetFormat) -> &'static str {
    match format {
        ImageAssetFormat::Png => "image/png",
        ImageAssetFormat::Jpeg => "image/jpeg",
        ImageAssetFormat::Webp => "image/webp",
        ImageAssetFormat::Gif => "image/gif",
        ImageAssetFormat::Svg => "image/svg+xml",
        ImageAssetFormat::Bmp => "image/bmp",
        ImageAssetFormat::Tiff => "image/tiff",
    }
}

fn media_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
