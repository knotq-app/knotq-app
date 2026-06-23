use super::super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn block_object_caret_bounds(
        &self,
        loc: TextLocation,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        if let Some(object) = self.table_object_range_for_row(loc.row) {
            if loc.col == object.start || loc.col == object.end {
                let (origin, grid_w, grid_h) = self.table_grid_geom(loc.row, bounds)?;
                let x = if loc.col == object.end {
                    origin.x + grid_w
                } else {
                    origin.x
                };
                return Some(Bounds::new(
                    point(x, origin.y),
                    size(px(1.5), grid_h.max(px(12.0))),
                ));
            }
        }
        self.image_object_caret_bounds(loc, bounds)
    }

    fn image_object_caret_bounds(
        &self,
        loc: TextLocation,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let (object, image_index) = self.image_object_at_location(loc)?;
        let image = self.image_bounds_for_index(loc.row, image_index, bounds)?;
        let x = if loc.col == object.end {
            image.right()
        } else {
            image.left()
        };
        Some(Bounds::new(
            point(x, image.top()),
            size(px(1.5), image.size.height.max(px(12.0))),
        ))
    }

    pub(in crate::scheme_editor) fn image_object_at_location(
        &self,
        loc: TextLocation,
    ) -> Option<(Range<usize>, usize)> {
        let editor_row = self.rows.get(loc.row)?;
        if editor_row.path.is_cell() || editor_row.path.is_table_anchor() {
            return None;
        }
        let line = self
            .line_range(loc.row)
            .and_then(|range| self.text.get(range))?;
        let mut block_index = 0;
        let mut image_index = 0;
        let inlines = editor_row.item.content.to_inlines();
        for object in block_object_ranges(line) {
            let inline = inlines
                .iter()
                .filter(|inline| !inline.is_text())
                .nth(block_index)?;
            let is_here = loc.col == object.start || loc.col == object.end;
            match inline {
                Inline::Image(_) if is_here => return Some((object, image_index)),
                Inline::Image(_) => image_index += 1,
                Inline::Table(_) => {}
                Inline::Text { .. } => unreachable!(),
            }
            block_index += 1;
        }
        None
    }

    fn image_bounds_for_index(
        &self,
        row: usize,
        target_image_index: usize,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let image = self.image_bounds_for_index_content(row, target_image_index)?;
        Some(Bounds::new(
            self.content_to_window(image.origin, bounds),
            image.size,
        ))
    }

    pub(in crate::scheme_editor) fn image_bounds_for_index_content(
        &self,
        row: usize,
        target_image_index: usize,
    ) -> Option<Bounds<Pixels>> {
        let editor_row = self.rows.get(row)?;
        let item_line = self.line_map.item_line(row)?;
        let max_width = self.image_max_width_for_row(row, item_line);
        let line = self
            .line_range(row)
            .and_then(|range| self.text.get(range))?;
        let has_text = !clean_line_text(line).is_empty();
        let (base_x, base_y) = self.row_base_xy(row);
        let annotation_height = item_line
            .annotation
            .as_ref()
            .map(|annotation| annotation.height)
            .unwrap_or(px(0.0));
        let mut y = base_y
            + self.line_map.line_text_height(row)
            + annotation_height
            + if has_text { px(IMAGE_TOP_GAP) } else { px(0.0) };
        let text_left = base_x + self.first_text_x(row);
        let mut image_index = 0;
        for media in editor_row.item.images() {
            let media_size = media_display_size(media, max_width);
            if media_size.height <= px(0.0) {
                continue;
            }
            let image = Bounds::new(point(text_left, y), media_size);
            if image_index == target_image_index {
                return Some(image);
            }
            y += media_size.height + px(IMAGE_STACK_GAP);
            image_index += 1;
        }
        None
    }

    pub(in crate::scheme_editor) fn image_max_width_for_row(&self, row: usize, line: &SchemeItemLine) -> Pixels {
        let text_width = line.text.size(line.line_height()).width.max(px(120.0));
        (self
            .last_bounds
            .map(|bounds| bounds.size.width - px(TEXT_LEFT_PAD + 24.0) - self.row_indent_x(row))
            .unwrap_or(text_width))
        .max(px(120.0))
    }

    pub(in crate::scheme_editor) fn marker_left_for_text_left(&self, item: &Item, text_left: Pixels) -> Pixels {
        if item.marker == ItemMarker::Blank {
            text_left
        } else {
            text_left - px(CHECKBOX_SIZE + CHECKBOX_GAP)
        }
    }

    pub(in crate::scheme_editor) fn media_stack_height(
        &self,
        item: &Item,
        max_width: Pixels,
        has_text: bool,
    ) -> Pixels {
        let mut height = px(0.0);
        let mut count = 0;
        for media in item.images() {
            let media_size = media_display_size(media, max_width);
            if media_size.height <= px(0.0) {
                continue;
            }
            if count == 0 {
                // An image-only line has no text to separate from, so it sits
                // flush at the top of the (collapsed) line band.
                height += if has_text { px(IMAGE_TOP_GAP) } else { px(0.0) };
            } else {
                height += px(IMAGE_STACK_GAP);
            }
            height += media_size.height;
            count += 1;
        }
        height
    }

    pub(in crate::scheme_editor) fn image_for_media(&mut self, media: &ImageInline) -> Option<Arc<Image>> {
        if let Some(cached) = self.image_cache.get(&media.asset) {
            return cached.clone();
        }

        // Cache the result either way: a missing asset stays `None` so we don't
        // hit the disk again on every repaint.
        let image = load_image_for_media(media);
        self.image_cache.insert(media.asset, image.clone());
        image
    }

    /// Forget cached image-load failures so assets that have since appeared on
    /// disk (e.g. downloaded by sync) are retried on the next paint. Successful
    /// loads stay cached because assets are immutable (keyed by a fresh UUID).
    pub fn forget_missing_images(&mut self) {
        self.image_cache.retain(|_, image| image.is_some());
    }
}
