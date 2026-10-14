use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::assets::image_asset_path;
use gpui::{
    px, size, ClipboardEntry, ClipboardItem, Image, ImageFormat as GpuiImageFormat, Pixels,
};
use image::GenericImageView;
use knotq_model::{ImageAssetFormat, ImageInline};
use uuid::Uuid;

use super::{IMAGE_FALLBACK_HEIGHT, IMAGE_FALLBACK_WIDTH, IMAGE_MAX_HEIGHT};

pub(super) const MAX_IMAGE_ASSET_BYTES: usize = 3 * 1024 * 1024;

/// The smallest a dragged image may get, in points, on either side.
pub(super) const IMAGE_MIN_SIDE: f32 = 32.0;
/// Side of the square at an image's bottom-right corner that starts a resize.
pub(super) const IMAGE_RESIZE_HANDLE: f32 = 16.0;

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
            let label = name
                .as_deref()
                .unwrap_or_else(|| knotq_l10n::t("editor.media.fallback_label"));
            knotq_l10n::t_with(
                "editor.media.rejected_line",
                &[("label", label), ("reason", &short_media_reason(error))],
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    knotq_l10n::t_with(
        "editor.media.rejected_count",
        &[("count", &rejections.len().to_string()), ("lines", &lines)],
    )
}

fn single_media_rejection_message(name: Option<&str>, error: &MediaError) -> String {
    let subject = match name {
        Some(name) => knotq_l10n::t_with("editor.media.subject_named", &[("name", name)]),
        None => knotq_l10n::t("editor.media.subject_generic").to_string(),
    };
    match error {
        MediaError::TooLarge { bytes } => knotq_l10n::t_with(
            "editor.media.too_large",
            &[
                ("subject", &subject),
                ("size", &megabytes(*bytes)),
                ("limit", &megabytes(MAX_IMAGE_ASSET_BYTES)),
            ],
        ),
        MediaError::UnsupportedFormat => {
            knotq_l10n::t_with("editor.media.unsupported_format", &[("subject", &subject)])
        }
        MediaError::IoFailed => {
            knotq_l10n::t_with("editor.media.io_failed", &[("subject", &subject)])
        }
    }
}

fn short_media_reason(error: &MediaError) -> String {
    match error {
        MediaError::TooLarge { bytes } => knotq_l10n::t_with(
            "editor.media.reason_too_large",
            &[
                ("size", &megabytes(*bytes)),
                ("limit", &megabytes(MAX_IMAGE_ASSET_BYTES)),
            ],
        ),
        MediaError::UnsupportedFormat => {
            knotq_l10n::t("editor.media.reason_unsupported").to_string()
        }
        MediaError::IoFailed => knotq_l10n::t("editor.media.reason_io_failed").to_string(),
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

/// The size an image is drawn at. `width`/`height` on the line are its size in
/// points; an image nobody has resized is additionally held to
/// [`IMAGE_MAX_HEIGHT`] so a tall screenshot doesn't take over the page, while a
/// size the user dragged to is honoured up to the width of the line.
pub(super) fn media_display_size(
    media: &ImageInline,
    max_width: Pixels,
    user_sized: bool,
) -> gpui::Size<Pixels> {
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
    let height_scale = if user_sized {
        1.0
    } else {
        IMAGE_MAX_HEIGHT / raw_height
    };
    let scale = (max_width / raw_width).min(height_scale).clamp(0.05, 1.0);
    size(px(raw_width * scale), px(raw_height * scale))
}

/// Whether the size on the line is one the user dragged to rather than the
/// file's own pixel size.
///
/// A resize is stored by rewriting `width`/`height` — the two numbers every
/// build on every platform already lays an image out from — so it reaches older
/// desktop and mobile builds with no format change. That leaves no flag to
/// read, so a resize is recognised by what it produces: a size that differs
/// from the file's while keeping its proportions. Anything else (the file's own
/// size, or proportions that disagree with the file) is laid out as before.
pub(super) fn media_is_user_sized(media: &ImageInline, intrinsic: Option<(u32, u32)>) -> bool {
    let (Some(width), Some(height)) = (media.width, media.height) else {
        return false;
    };
    let Some((intrinsic_width, intrinsic_height)) = intrinsic else {
        // An SVG has no pixel size, so it is stored without one until resized.
        return media.format == ImageAssetFormat::Svg;
    };
    if intrinsic_width == 0 || intrinsic_height == 0 {
        return false;
    }
    if (width, height) == (intrinsic_width, intrinsic_height) {
        return false;
    }
    let proportional_height = width as f64 * intrinsic_height as f64 / intrinsic_width as f64;
    (proportional_height - height as f64).abs() <= 1.0
}

/// `media` resized to `width` points, keeping the proportions of the file (or
/// of the stored size when the file's are unknown).
pub(super) fn media_resized_to_width(
    media: &ImageInline,
    intrinsic: Option<(u32, u32)>,
    width: f32,
) -> ImageInline {
    let (aspect_width, aspect_height) = intrinsic
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| (width as f64, height as f64))
        .unwrap_or((
            media
                .width
                .map(f64::from)
                .unwrap_or(IMAGE_FALLBACK_WIDTH as f64),
            media
                .height
                .map(f64::from)
                .unwrap_or(IMAGE_FALLBACK_HEIGHT as f64),
        ));
    let height_for =
        |width: u32| ((width as f64 * aspect_height / aspect_width).round() as u32).max(1);
    let mut width = (width.round() as u32).max(1);
    // Landing exactly on the file's own size would read as "never resized" and
    // bring the height cap back; one point narrower keeps it a chosen size.
    if intrinsic == Some((width, height_for(width))) && width > 1 {
        width -= 1;
    }
    ImageInline {
        width: Some(width),
        height: Some(height_for(width)),
        ..*media
    }
}

/// The pixel size recorded in the image file itself, read from its header.
pub(super) fn image_intrinsic_size(media: &ImageInline) -> Option<(u32, u32)> {
    let format = decodable_image_format(media.format)?;
    let file = fs::File::open(image_asset_path(media.asset, media.format.extension())).ok()?;
    image::ImageReader::with_format(std::io::BufReader::new(file), format)
        .into_dimensions()
        .ok()
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

fn decodable_image_format(format: ImageAssetFormat) -> Option<image::ImageFormat> {
    Some(match format {
        ImageAssetFormat::Png => image::ImageFormat::Png,
        ImageAssetFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageAssetFormat::Webp => image::ImageFormat::WebP,
        ImageAssetFormat::Gif => image::ImageFormat::Gif,
        ImageAssetFormat::Bmp => image::ImageFormat::Bmp,
        ImageAssetFormat::Tiff => image::ImageFormat::Tiff,
        ImageAssetFormat::Svg => return None,
    })
}

fn image_dimensions(format: ImageAssetFormat, bytes: &[u8]) -> (Option<u32>, Option<u32>) {
    let Some(image_format) = decodable_image_format(format) else {
        return (None, None);
    };
    image::load_from_memory_with_format(bytes, image_format)
        .ok()
        .map(|image| {
            let (width, height) = image.dimensions();
            (Some(width), Some(height))
        })
        .unwrap_or((None, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> ImageInline {
        ImageInline {
            asset: Uuid::from_u128(7),
            format: ImageAssetFormat::Png,
            width: Some(width),
            height: Some(height),
        }
    }

    fn shown(media: &ImageInline, max_width: f32, intrinsic: Option<(u32, u32)>) -> (f32, f32) {
        let size = media_display_size(media, px(max_width), media_is_user_sized(media, intrinsic));
        (size.width.to_f64() as f32, size.height.to_f64() as f32)
    }

    #[test]
    fn an_untouched_image_is_still_held_to_the_height_cap() {
        let tall = image(1170, 2532);
        let (width, height) = shown(&tall, 700.0, Some((1170, 2532)));
        assert_eq!(height, IMAGE_MAX_HEIGHT);
        assert!((width - 1170.0 * IMAGE_MAX_HEIGHT / 2532.0).abs() < 0.01);
    }

    #[test]
    fn a_resized_image_may_be_taller_than_the_cap_but_never_wider_than_the_line() {
        let intrinsic = Some((1170, 2532));
        let resized = media_resized_to_width(&image(1170, 2532), intrinsic, 400.0);
        assert_eq!(resized.width, Some(400));
        assert_eq!(resized.height, Some(866));
        assert!(media_is_user_sized(&resized, intrinsic));
        assert_eq!(shown(&resized, 700.0, intrinsic), (400.0, 866.0));

        // A narrower window scales it down, proportions intact.
        let (width, height) = shown(&resized, 200.0, intrinsic);
        assert_eq!(width, 200.0);
        assert_eq!(height, 433.0);
    }

    #[test]
    fn a_resize_keeps_the_files_proportions_however_many_times_it_is_dragged() {
        let intrinsic = Some((1170, 2532));
        let mut media = image(1170, 2532);
        for width in [40.0, 613.0, 77.0, 999.0, 41.0, 650.0] {
            media = media_resized_to_width(&media, intrinsic, width);
            assert!(media_is_user_sized(&media, intrinsic), "{media:?}");
        }
        assert_eq!((media.width, media.height), (Some(650), Some(1407)));
    }

    #[test]
    fn dragging_back_to_the_files_own_size_stays_a_chosen_size() {
        let intrinsic = Some((600, 900));
        let resized = media_resized_to_width(&image(300, 450), intrinsic, 600.0);
        assert_eq!((resized.width, resized.height), (Some(599), Some(899)));
        assert!(media_is_user_sized(&resized, intrinsic));
    }

    #[test]
    fn a_size_whose_proportions_disagree_with_the_file_is_not_a_resize() {
        // What a phone photo re-encoded with its rotation baked in looks like:
        // the stored size is the file's with the sides swapped.
        assert!(!media_is_user_sized(&image(4032, 3024), Some((3024, 4032))));
        // And with no file to compare against, nothing is assumed.
        assert!(!media_is_user_sized(&image(400, 866), None));
    }

    #[test]
    fn a_resized_svg_is_recognised_without_a_pixel_size() {
        let svg = ImageInline {
            format: ImageAssetFormat::Svg,
            width: None,
            height: None,
            ..image(0, 0)
        };
        assert!(!media_is_user_sized(&svg, None));
        let resized = media_resized_to_width(&svg, None, 640.0);
        assert_eq!((resized.width, resized.height), (Some(640), Some(360)));
        assert!(media_is_user_sized(&resized, None));
    }
}
