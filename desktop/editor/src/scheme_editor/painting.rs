use super::*;

impl SchemeEditor {
    pub(super) fn paint_editor(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.last_bounds = Some(bounds);
        self.checkbox_hitboxes.clear();
        self.date_annotation_hitboxes.clear();
        self.repeat_annotation_hitboxes.clear();
        self.table_control_hitboxes.clear();
        let theme = self.theme;
        let text_origin = point(
            bounds.left() + px(TEXT_LEFT_PAD),
            bounds.top() + px(self.top_pad),
        );
        let focused = !self.read_only && self.focus_handle.is_focused(window);
        let active_row = self
            .selection
            .head
            .row
            .min(self.render_line_count().saturating_sub(1));
        let active_in_cell = self
            .rows
            .get(active_row)
            .is_some_and(|row| row.path.is_cell());

        for row in 0..self.line_map.line_count() {
            if self
                .rows
                .get(row)
                .is_some_and(|row| row.path.is_table_anchor())
            {
                self.paint_table_chrome(row, bounds, window, cx);
            }
        }

        if focused && !active_in_cell {
            if let Some(row_bounds) = self.row_bounds(active_row, bounds) {
                window.paint_quad(fill(row_bounds, token_rgba(theme.row_hover)));
            }
        }

        // Run backgrounds (e.g. ==highlight==) are not drawn by `paint`, and must
        // sit *below* the selection — otherwise a selected highlight would paint
        // over the selection quad and the selection would be invisible on it.
        for row in 0..self.line_map.line_count() {
            if self
                .rows
                .get(row)
                .is_some_and(|row| row.path.is_table_anchor())
            {
                continue;
            }
            let (base_x, base_y) = self.row_base_xy(row);
            if let Some(line) = self.line_map.line(row).cloned() {
                let line_origin = point(text_origin.x + base_x, text_origin.y + base_y);
                let line_height = self.line_map.row_line_height(row);
                let _ = line.paint_background(
                    line_origin,
                    line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }

        if focused && !self.selection.is_empty() {
            self.paint_selection(text_origin, window);
        }

        for row in 0..self.line_map.line_count() {
            let path = self.rows.get(row).map(|row| row.path).unwrap_or_default();
            // A table anchor's grid chrome is painted earlier, but its *text*
            // (a title/caption on the same line as the table) still renders here
            // in the line band above the grid — otherwise that text is invisible.
            let (base_x, base_y) = self.row_base_xy(row);
            if let Some(line) = self.line_map.line(row).cloned() {
                let line_origin = point(text_origin.x + base_x, text_origin.y + base_y);
                let line_height = self.line_map.row_line_height(row);
                let _ = line.paint(line_origin, line_height, TextAlign::Left, None, window, cx);
                if !path.is_cell() {
                    self.paint_block_suffix(row, text_origin, window, cx);
                }
                if let Some(editor_row) = self.rows.get(row).cloned() {
                    self.paint_line_marker(&editor_row, row, line_origin, window, cx);
                    if self
                        .line_map
                        .item_line(row)
                        .and_then(|line| line.annotation.as_ref())
                        .is_some()
                    {
                        self.paint_date_annotation(&editor_row, row, line_origin, window, cx);
                    }
                    if !path.is_cell() && !path.is_table_anchor() {
                        self.paint_item_media(&editor_row.item, row, line_origin, window, cx);
                    }
                }
            }
        }

        if focused && self.selection.is_empty() && self.cursor_blink_state {
            if let Some(caret) = self.block_object_caret_bounds(self.selection.head, bounds) {
                window.paint_quad(fill(caret, token_hsla(theme.caret_color)));
                return;
            }
            let pos = self.visual_point_for_location(self.selection.head);
            let row_height = self.line_map.row_line_height(self.selection.head.row);
            // Scale the caret with the line so it grows on larger heading rows.
            let caret_height = (row_height - px(4.0)).max(px(12.0));
            let caret_top_offset = ((row_height - caret_height) / 2.0).max(px(0.0));
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        text_origin.x + pos.x,
                        text_origin.y + pos.y + caret_top_offset,
                    ),
                    size(px(1.5), caret_height),
                ),
                token_hsla(theme.caret_color),
            ));
        }
    }

    fn paint_block_suffix(
        &self,
        row: usize,
        text_origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(item_line) = self.line_map.item_line(row) else {
            return;
        };
        let Some(suffix) = item_line.block_suffix.as_ref() else {
            return;
        };
        let Some(origin) = self.block_suffix_origin(row, text_origin, item_line) else {
            return;
        };
        let _ = suffix.paint(
            origin,
            self.line_map.row_line_height(row),
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    fn block_suffix_origin(
        &self,
        row: usize,
        text_origin: Point<Pixels>,
        item_line: &SchemeItemLine,
    ) -> Option<Point<Pixels>> {
        if let Some(layout) = self.table_layouts.get(&row) {
            let line_top = self.line_map.y_range(row..row + 1).start;
            return Some(point(
                text_origin.x + self.table_grid_left_content(row),
                text_origin.y
                    + line_top
                    + self.line_map.line_text_height(row)
                    + layout.block_height
                    + item_line.block_suffix_gap,
            ));
        }

        let editor_row = self.rows.get(row)?;
        if !editor_row.item.has_images() {
            return None;
        }
        let (base_x, base_y) = self.row_base_xy(row);
        let max_width = self.image_max_width_for_row(row, item_line);
        let line = self
            .line_range(row)
            .and_then(|range| self.text.get(range))?;
        let has_text = !clean_line_text(line).is_empty();
        let annotation_height = item_line
            .annotation
            .as_ref()
            .map(|annotation| annotation.height)
            .unwrap_or(px(0.0));
        let media_height = self.media_stack_height(&editor_row.item, max_width, has_text);
        if media_height <= px(0.0) {
            return None;
        }
        Some(point(
            text_origin.x + base_x + self.first_text_x(row),
            text_origin.y
                + base_y
                + self.line_map.line_text_height(row)
                + annotation_height
                + media_height
                + item_line.block_suffix_gap,
        ))
    }

    fn block_object_caret_bounds(
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

    pub(super) fn image_object_at_location(
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
        for object in block_object_ranges(line) {
            let inline = editor_row
                .item
                .content
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

    pub(super) fn image_bounds_for_index_content(
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

    pub(super) fn image_max_width_for_row(&self, row: usize, line: &SchemeItemLine) -> Pixels {
        let text_width = line.text.size(line.line_height()).width.max(px(120.0));
        (self
            .last_bounds
            .map(|bounds| bounds.size.width - px(TEXT_LEFT_PAD + 24.0) - self.row_indent_x(row))
            .unwrap_or(text_width))
        .max(px(120.0))
    }

    pub(super) fn marker_left_for_text_left(&self, item: &Item, text_left: Pixels) -> Pixels {
        if item.marker == ItemMarker::Blank {
            text_left
        } else {
            text_left - px(CHECKBOX_SIZE + CHECKBOX_GAP)
        }
    }

    pub(super) fn media_stack_height(
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

    pub(super) fn paint_item_media(
        &mut self,
        item: &Item,
        row_ix: usize,
        line_origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !item.has_images() {
            return;
        }

        let Some(line) = self.line_map.item_line(row_ix) else {
            return;
        };
        let text_width = line.text.size(line.line_height()).width.max(px(120.0));
        let max_width = (self
            .last_bounds
            .map(|bounds| bounds.size.width - px(TEXT_LEFT_PAD + 24.0) - self.row_indent_x(row_ix))
            .unwrap_or(text_width))
        .max(px(120.0));

        let text_left = line_origin.x + self.first_text_x(row_ix);
        let annotation_height = line
            .annotation
            .as_ref()
            .map(|annotation| annotation.height)
            .unwrap_or(px(0.0));
        let has_text = !clean_line_text(&item.text()).is_empty();
        let mut y = line_origin.y
            + self.line_map.line_text_height(row_ix)
            + annotation_height
            + if has_text { px(IMAGE_TOP_GAP) } else { px(0.0) };

        for media in item.images() {
            let media_size = media_display_size(media, max_width);
            if media_size.height <= px(0.0) {
                continue;
            }
            let bounds = Bounds::new(point(text_left, y), media_size);
            window.paint_quad(quad(
                bounds,
                px(5.0),
                token_rgba(self.theme.button_bg),
                px(1.0),
                token_hsla(self.theme.border_main),
                BorderStyle::default(),
            ));

            if let Some(image) = self.image_for_media(media) {
                if let Some(render_image) = image.get_render_image(window, cx) {
                    let _ =
                        window.paint_image(bounds, Corners::all(px(5.0)), render_image, 0, false);
                }
            }
            y += media_size.height + px(IMAGE_STACK_GAP);
        }
    }

    pub(super) fn image_for_media(&mut self, media: &ImageInline) -> Option<Arc<Image>> {
        if let Some(cached) = self.image_cache.get(&media.asset) {
            return cached.clone();
        }

        let image = load_image_for_media(media);
        if image.is_some() {
            self.image_cache.insert(media.asset, image.clone());
        }
        image
    }
}
