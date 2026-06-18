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
