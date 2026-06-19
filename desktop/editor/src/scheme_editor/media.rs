use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::assets::image_asset_path;
use gpui::{
    px, size, ClipboardEntry, ClipboardItem, Image, ImageFormat as GpuiImageFormat,
    Pixels,
};
use image::GenericImageView;
use knotq_model::{ImageAssetFormat, ImageInline};
use uuid::Uuid;

use super::{IMAGE_FALLBACK_HEIGHT, IMAGE_FALLBACK_WIDTH, IMAGE_MAX_HEIGHT};

pub(super) const MAX_IMAGE_ASSET_BYTES: usize = 3 * 1024 * 1024;

/// Why an image couldn't be added, so the host can tell the user instead of
/// failing silently.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum MediaError {
    /// The format isn't one we can store/render.
    UnsupportedFormat,
    /// The source bytes exceed [`MAX_IMAGE_ASSET_BYTES`].
    TooLarge { bytes: usize },
    /// Reading the source file or writing the asset failed.
    IoFailed,
}

pub(super) fn clipboard_image(item: &ClipboardItem) -> Option<&Image> {
    item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image),
        ClipboardEntry::String(_) => None,
    })
}

pub(super) fn persist_clipboard_image(image: &Image) -> Result<ImageInline, MediaError> {
    let Some(format) = image_asset_format(image.format()) else {
        return Err(MediaError::UnsupportedFormat);
    };
    persist_image_bytes(format, image.bytes())
}

pub(super) fn persist_image_file(path: &Path) -> Result<ImageInline, MediaError> {
    let Some(format) = image_asset_format_from_path(path) else {
        return Err(MediaError::UnsupportedFormat);
    };
    let bytes = fs::read(path).map_err(|err| {
        eprintln!("image drop failed to read {}: {err}", path.display());
        MediaError::IoFailed
    })?;
    persist_image_bytes(format, &bytes)
}

fn persist_image_bytes(format: ImageAssetFormat, bytes: &[u8]) -> Result<ImageInline, MediaError> {
    if bytes.len() > MAX_IMAGE_ASSET_BYTES {
        return Err(MediaError::TooLarge { bytes: bytes.len() });
    }
    let asset = Uuid::new_v4();
    let out_path = image_asset_path(asset, format.extension());
    if let Some(parent) = out_path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            eprintln!("image save failed to create {}: {err}", parent.display());
            return Err(MediaError::IoFailed);
        }
    }
    if let Err(err) = fs::write(&out_path, bytes) {
        eprintln!("image save failed to write {}: {err}", out_path.display());
        return Err(MediaError::IoFailed);
    }
    let (width, height) = image_dimensions(format, bytes);
    Ok(ImageInline {
        asset,
        format,
        width,
        height,
    })
}

/// A user-facing explanation for one or more rejected images, paired with the
/// source file name when known (clipboard pastes have none).
pub(super) fn media_rejection_message(rejections: &[(Option<String>, MediaError)]) -> String {
    if let [(name, error)] = rejections {
        return single_media_rejection_message(name.as_deref(), error);
    }
    let lines = rejections
        .iter()
        .map(|(name, error)| {
            let label = name.as_deref().unwrap_or("Image");
            format!("\u{2022} {label}: {}", short_media_reason(error))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{} images couldn't be added:\n{lines}", rejections.len())
}

fn single_media_rejection_message(name: Option<&str>, error: &MediaError) -> String {
    let subject = match name {
        Some(name) => format!("'{name}'"),
        None => "That image".to_string(),
    };
    match error {
        MediaError::TooLarge { bytes } => format!(
            "{subject} is {} - images must be under {} to be added.",
            megabytes(*bytes),
            megabytes(MAX_IMAGE_ASSET_BYTES)
        ),
        MediaError::UnsupportedFormat => format!("{subject} isn't a supported image format."),
        MediaError::IoFailed => format!("{subject} couldn't be saved."),
    }
}

fn short_media_reason(error: &MediaError) -> String {
    match error {
        MediaError::TooLarge { bytes } => {
            format!("{} (limit {})", megabytes(*bytes), megabytes(MAX_IMAGE_ASSET_BYTES))
        }
        MediaError::UnsupportedFormat => "unsupported format".to_string(),
        MediaError::IoFailed => "couldn't be saved".to_string(),
    }
}

fn megabytes(bytes: usize) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
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
