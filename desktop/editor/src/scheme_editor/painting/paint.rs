use super::super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn paint_editor(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.last_bounds = Some(bounds);
        self.checkbox_hitboxes.clear();
        self.date_annotation_hitboxes.clear();
        self.repeat_annotation_hitboxes.clear();
        self.link_hitboxes.clear();
        self.open_link_button = None;
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
                self.register_link_hitboxes(row, line_origin);
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

        if focused && !self.selection.is_empty() {
            self.paint_block_object_selection(bounds, window);
        }

        if focused {
            self.paint_link_open_button(bounds, text_origin, window, cx);
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

        // Remote peers' carets (multiplayer presence). Painted regardless of local
        // focus/blink so collaborators are always visible, and located by item id
        // so they're correct under this device's own layout.
        self.paint_remote_cursors(text_origin, window);
    }

    fn paint_remote_cursors(&self, text_origin: Point<Pixels>, window: &mut Window) {
        if self.remote_cursors.is_empty() {
            return;
        }
        for cursor in &self.remote_cursors {
            let Some(row) = self.rows.iter().position(|r| r.item.id == cursor.item_id) else {
                continue;
            };
            if row >= self.line_map.line_count() {
                continue;
            }
            let loc = self.clamp_location(TextLocation {
                row,
                col: cursor.col,
            });
            let pos = self.visual_point_for_location(loc);
            let row_height = self.line_map.row_line_height(loc.row);
            let caret_height = (row_height - px(4.0)).max(px(12.0));
            let caret_top_offset = ((row_height - caret_height) / 2.0).max(px(0.0));
            let color = token_hsla(cursor.color);
            let x = text_origin.x + pos.x;
            let y = text_origin.y + pos.y + caret_top_offset;
            // A 2px caret bar, slightly wider than the local 1.5px caret.
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(px(2.0), caret_height)),
                color,
            ));
            // A small flag at the top so the peer caret reads as "someone is here".
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(px(6.0), px(4.0))),
                color,
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

    pub(in crate::scheme_editor) fn paint_item_media(
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
}
