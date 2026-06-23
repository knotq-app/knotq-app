use super::*;

mod copy;
mod media;
mod paste;

pub(super) fn insert_images_at_text_col(item: &mut Item, col: usize, images: Vec<ImageInline>) {
    let mut remaining = col;
    let mut inserted = false;
    let existing = std::mem::take(&mut item.content).to_inlines();
    let mut output = Vec::with_capacity(existing.len() + images.len());
    let mut image_inlines = images.into_iter().map(Inline::Image).collect::<Vec<_>>();

    for inline in existing {
        match inline {
            Inline::Text { text } if !inserted => {
                if remaining <= text.len() {
                    let split = previous_char_boundary_at(&text, remaining);
                    if split > 0 {
                        output.push(Inline::text(text[..split].to_string()));
                    }
                    output.append(&mut image_inlines);
                    if split < text.len() {
                        output.push(Inline::text(text[split..].to_string()));
                    }
                    inserted = true;
                } else {
                    remaining = remaining.saturating_sub(text.len());
                    output.push(Inline::text(text));
                }
            }
            other => output.push(other),
        }
    }

    if !inserted {
        output.append(&mut image_inlines);
    }

    item.content = ItemContent::from_inlines(output);
}

pub(super) fn previous_char_boundary_at(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Persist a batch of image files, splitting them into the stored media and the
/// per-file rejections (with their source file name) to report to the user.
pub(super) fn persist_image_files(
    paths: &[std::path::PathBuf],
) -> (Vec<ImageInline>, Vec<(Option<String>, MediaError)>) {
    let mut media = Vec::new();
    let mut rejections = Vec::new();
    for path in paths {
        match persist_image_file(path) {
            Ok(inline) => media.push(inline),
            Err(error) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                rejections.push((name, error));
            }
        }
    }
    (media, rejections)
}
