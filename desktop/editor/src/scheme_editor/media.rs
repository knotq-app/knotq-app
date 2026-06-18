use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::assets::image_asset_path;
use gpui::{
    px, size, ClipboardEntry, ClipboardItem, ExternalPaths, Image, ImageFormat as GpuiImageFormat,
    Pixels,
};
use image::GenericImageView;
use knotq_model::{ImageAssetFormat, ImageInline};
use uuid::Uuid;

use super::{IMAGE_FALLBACK_HEIGHT, IMAGE_FALLBACK_WIDTH, IMAGE_MAX_HEIGHT};

const MAX_IMAGE_ASSET_BYTES: usize = 3 * 1024 * 1024;

pub(super) fn clipboard_image(item: &ClipboardItem) -> Option<&Image> {
    item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image),
        ClipboardEntry::String(_) => None,
    })
}

pub(super) fn persist_clipboard_image(image: &Image) -> Option<ImageInline> {
    let format = image_asset_format(image.format())?;
    if image.bytes().len() > MAX_IMAGE_ASSET_BYTES {
        eprintln!(
            "image paste rejected: {} bytes exceeds {} byte sync limit",
            image.bytes().len(),
            MAX_IMAGE_ASSET_BYTES
        );
        return None;
    }
    let asset = Uuid::new_v4();
    let path = image_asset_path(asset, format.extension());
    if let Some(parent) = path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            eprintln!("image paste failed to create {}: {err}", parent.display());
            return None;
        }
    }
    if let Err(err) = fs::write(&path, image.bytes()) {
        eprintln!("image paste failed to write {}: {err}", path.display());
        return None;
    }
    let (width, height) = image_dimensions(format, image.bytes());
    Some(ImageInline {
        asset,
        format,
        width,
        height,
    })
}

pub(super) fn external_paths_have_supported_image(paths: &ExternalPaths) -> bool {
    paths
        .paths()
        .iter()
        .any(|path| image_asset_format_from_path(path).is_some())
}

pub(super) fn persist_image_file(path: &Path) -> Option<ImageInline> {
    let format = image_asset_format_from_path(path)?;
    let bytes = fs::read(path).ok()?;
    if bytes.len() > MAX_IMAGE_ASSET_BYTES {
        eprintln!(
            "image drop rejected: {} bytes exceeds {} byte sync limit",
            bytes.len(),
            MAX_IMAGE_ASSET_BYTES
        );
        return None;
    }
    let asset = Uuid::new_v4();
    let out_path = image_asset_path(asset, format.extension());
    if let Some(parent) = out_path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            eprintln!("image drop failed to create {}: {err}", parent.display());
            return None;
        }
    }
    if let Err(err) = fs::write(&out_path, &bytes) {
        eprintln!("image drop failed to write {}: {err}", out_path.display());
        return None;
    }
    let (width, height) = image_dimensions(format, &bytes);
    Some(ImageInline {
        asset,
        format,
        width,
        height,
    })
}

pub(super) fn gpui_image_format(format: ImageAssetFormat) -> GpuiImageFormat {
    match format {
        ImageAssetFormat::Png => GpuiImageFormat::Png,
        ImageAssetFormat::Jpeg => GpuiImageFormat::Jpeg,
        ImageAssetFormat::Webp => GpuiImageFormat::Webp,
        ImageAssetFormat::Gif => GpuiImageFormat::Gif,
        ImageAssetFormat::Svg => GpuiImageFormat::Svg,
        ImageAssetFormat::Bmp => GpuiImageFormat::Bmp,
        ImageAssetFormat::Tiff => GpuiImageFormat::Tiff,
    }
}

pub(super) fn media_display_size(media: &ImageInline, max_width: Pixels) -> gpui::Size<Pixels> {
    let raw_width = media
        .width
        .map(|width| width as f32)
        .unwrap_or(IMAGE_FALLBACK_WIDTH);
    let raw_height = media
        .height
        .map(|height| height as f32)
        .unwrap_or(IMAGE_FALLBACK_HEIGHT);
    if raw_width <= 0.0 || raw_height <= 0.0 {
        return size(px(0.0), px(0.0));
    }
    let max_width = max_width.to_f64() as f32;
    let scale = (max_width / raw_width)
        .min(IMAGE_MAX_HEIGHT / raw_height)
        .clamp(0.05, 1.0);
    size(px(raw_width * scale), px(raw_height * scale))
}

pub(super) fn load_image_for_media(media: &ImageInline) -> Option<Arc<Image>> {
    fs::read(image_asset_path(media.asset, media.format.extension()))
        .ok()
        .map(|bytes| Arc::new(Image::from_bytes(gpui_image_format(media.format), bytes)))
}

fn image_asset_format(format: GpuiImageFormat) -> Option<ImageAssetFormat> {
    Some(match format {
        GpuiImageFormat::Png => ImageAssetFormat::Png,
        GpuiImageFormat::Jpeg => ImageAssetFormat::Jpeg,
        GpuiImageFormat::Webp => ImageAssetFormat::Webp,
        GpuiImageFormat::Gif => ImageAssetFormat::Gif,
        GpuiImageFormat::Svg => ImageAssetFormat::Svg,
        GpuiImageFormat::Bmp => ImageAssetFormat::Bmp,
        GpuiImageFormat::Tiff => ImageAssetFormat::Tiff,
    })
}

fn image_asset_format_from_path(path: &Path) -> Option<ImageAssetFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => ImageAssetFormat::Png,
        "jpg" | "jpeg" => ImageAssetFormat::Jpeg,
        "webp" => ImageAssetFormat::Webp,
        "gif" => ImageAssetFormat::Gif,
        "svg" => ImageAssetFormat::Svg,
        "bmp" => ImageAssetFormat::Bmp,
        "tif" | "tiff" => ImageAssetFormat::Tiff,
        _ => return None,
    })
}

fn image_dimensions(format: ImageAssetFormat, bytes: &[u8]) -> (Option<u32>, Option<u32>) {
    let image_format = match format {
        ImageAssetFormat::Png => image::ImageFormat::Png,
        ImageAssetFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageAssetFormat::Webp => image::ImageFormat::WebP,
        ImageAssetFormat::Gif => image::ImageFormat::Gif,
        ImageAssetFormat::Bmp => image::ImageFormat::Bmp,
        ImageAssetFormat::Tiff => image::ImageFormat::Tiff,
        ImageAssetFormat::Svg => return (None, None),
    };
    image::load_from_memory_with_format(bytes, image_format)
        .ok()
        .map(|image| {
            let (width, height) = image.dimensions();
            (Some(width), Some(height))
        })
        .unwrap_or((None, None))
}
