use std::ops::Range;

use gpui::{fill, point, px, size, Bounds, Pixels, Point, Window};

use crate::line_map::{SchemeItemLine, TextLocation};
use crate::theme_gpui::text_selection_rgba;

use super::{
    block_object_ranges, buffer::HEADER_ROW, clean_line_text, SchemeEditor, ANNOTATION_BAR_GAP,
    CHECKBOX_GAP, CHECKBOX_SIZE, EMPTY_SELECTION_WIDTH, INDENT_WIDTH, MAX_INDENT, TEXT_LEFT_PAD,
};

impl SchemeEditor {
    pub(super) fn row_bounds(&self, row: usize, bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        if row >= self.line_map.line_count() {
            return None;
        }
        let y_range = self.line_map.y_range(row..row + 1);
        Some(Bounds::new(
            point(
                bounds.left(),
                bounds.top() + px(self.top_pad) + y_range.start,
            ),
            size(bounds.size.width, y_range.end - y_range.start),
        ))
    }

    pub(super) fn row_indent_x(&self, row: usize) -> Pixels {
        px(self
            .rows
            .get(row)
            .map(|row| row.item.indent.min(MAX_INDENT) as f32)
            .unwrap_or(0.0)
            * INDENT_WIDTH)
    }

    pub(super) fn row_layout_x(&self, row: usize) -> Pixels {
        self.row_indent_x(row) - self.row_plain_text_shift_x(row) - self.first_text_x(row)
    }

    pub(super) fn row_plain_text_shift_x(&self, row: usize) -> Pixels {
        if self
            .rows
            .get(row)
            .is_some_and(|row| row.item.marker == knotq_model::ItemMarker::Blank)
        {
            px(CHECKBOX_SIZE + CHECKBOX_GAP)
        } else {
            px(0.0)
        }
    }

    pub(super) fn first_text_x(&self, row: usize) -> Pixels {
        self.line_map
            .position_for_index(row, 0)
            .map(|point| point.x)
            .unwrap_or(px(0.0))
    }

    /// Content-space (relative to the text origin) base offset to add to a
    /// line's intra-line position. Cell rows live inside their table slot;
    /// ordinary rows keep the vertical flow position from the line map.
    pub(super) fn row_base_xy(&self, row: usize) -> (Pixels, Pixels) {
        if let Some(slot) = self.cell_slots.get(&row) {
            (slot.text_left + self.row_layout_x(row), slot.top)
        } else {
            (
                self.row_layout_x(row),
                self.line_map.y_range(row..row + 1).start,
            )
        }
    }

    pub(super) fn visual_point_for_location(&self, loc: TextLocation) -> Point<Pixels> {
        let loc = self.clamp_location(loc);
        if let Some(object) = self.table_object_range_for_row(loc.row) {
            if loc.col > object.end {
                if let Some(point) = self.block_suffix_point_for_location(loc, object.end) {
                    return point;
                }
            }
            if loc.col == object.start || loc.col == object.end {
                if let Some(layout) = self.table_layouts.get(&loc.row) {
                    let x = self.table_grid_left_content(loc.row)
                        + if loc.col == object.end {
                            layout.grid_w
                        } else {
                            px(0.0)
                        };
                    let y = self.line_map.y_range(loc.row..loc.row + 1).start
                        + self.line_map.line_text_height(loc.row)
                        + px(super::table::GRID_TOP_GAP);
                    return point(x, y);
                }
            }
        }
        if self
            .rows
            .get(loc.row)
            .is_some_and(|row| row.item.has_images())
        {
            if let Some(object) = self.last_block_object_range_for_row(loc.row) {
                if loc.col > object.end {
                    if let Some(point) = self.block_suffix_point_for_location(loc, object.end) {
                        return point;
                    }
                }
            }
        }
        if let Some((object, image_index)) = self.image_object_at_location(loc) {
            if loc.col == object.start || loc.col == object.end {
                if let Some(bounds) = self.image_bounds_for_index_content(loc.row, image_index) {
                    let x = if loc.col == object.end {
                        bounds.right()
                    } else {
                        bounds.left()
                    };
                    return point(x, bounds.top());
                }
            }
        }
        let intra = self
            .line_map
            .position_for_index(loc.row, loc.col)
            .unwrap_or_else(|| point(px(0.0), px(0.0)));
        let (base_x, base_y) = self.row_base_xy(loc.row);
        point(base_x + intra.x, base_y + intra.y)
    }

    fn block_suffix_point_for_location(
        &self,
        loc: TextLocation,
        suffix_start: usize,
    ) -> Option<Point<Pixels>> {
        let item_line = self.line_map.item_line(loc.row)?;
        let suffix = item_line.block_suffix.as_ref()?;
        let suffix_col = loc.col.saturating_sub(suffix_start);
        let suffix_pos = suffix
            .position_for_index(suffix_col, self.line_map.row_line_height(loc.row))
            .unwrap_or_else(|| point(px(0.0), px(0.0)));
        let origin = self.block_suffix_origin_content(loc.row, item_line)?;
        Some(point(origin.x + suffix_pos.x, origin.y + suffix_pos.y))
    }

    pub(super) fn location_for_local_point(&self, local: Point<Pixels>) -> TextLocation {
        if self.line_map.line_count() == 0 {
            return TextLocation { row: 0, col: 0 };
        }

        if let Some(loc) = self.cell_location_for_local_point(local) {
            return loc;
        }
        if let Some(loc) = self.block_suffix_location_for_local_point(local) {
            return loc;
        }
        // A click in the gutter immediately left/right of a table lands the caret
        // just before / just after the table.
        if let Some(loc) = self.table_gutter_location_for_local_point(local) {
            return loc;
        }

        // A click in the empty space below all content, when the last line is a
        // block (image/table), has no text line to land on and otherwise falls
        // into the block's last cell. Send it to the caret position right after
        // the block instead — where a precise click just past the block lands.
        if local.y >= self.line_map.total_height() {
            if let Some(loc) = self.after_trailing_block_location() {
                return loc;
            }
        }

        let probe = self.line_map.location_for_point(point(px(0.0), local.y));
        let row = probe.row.min(self.line_map.line_count().saturating_sub(1));
        self.clamp_location(
            self.line_map
                .location_for_point(point(local.x - self.row_layout_x(row), local.y)),
        )
    }

    fn cell_location_for_local_point(&self, local: Point<Pixels>) -> Option<TextLocation> {
        if let Some(loc) = self.cell_text_location_for_local_point(local) {
            return Some(loc);
        }

        self.cell_grid_location_for_local_point(local)
    }

    fn cell_text_location_for_local_point(&self, local: Point<Pixels>) -> Option<TextLocation> {
        let pad = px(super::table::CELL_PAD_X);
        let mut best: Option<(Pixels, TextLocation)> = None;

        for (&row, slot) in &self.cell_slots {
            let height = self
                .line_map
                .line_text_height(row)
                .max(self.line_map.row_line_height(row));
            let col_left = slot.col_left;
            let col_right = slot.col_left + slot.width + pad * 2.0;
            if local.y < slot.top || local.y >= slot.top + height {
                continue;
            }
            if local.x < col_left || local.x >= col_right {
                continue;
            }

            let base_x = slot.text_left + self.row_layout_x(row);
            let line_local = point(local.x - base_x, local.y - slot.top);
            let col = self.line_map.closest_col(row, line_local);
            let score = local.y - slot.top;
            let loc = self.clamp_location(TextLocation { row, col });
            if best.as_ref().is_none_or(|(current, _)| score < *current) {
                best = Some((score, loc));
            }
        }

        best.map(|(_, loc)| loc)
    }

    fn cell_grid_location_for_local_point(&self, local: Point<Pixels>) -> Option<TextLocation> {
        for (&anchor_row, layout) in &self.table_layouts {
            let grid_left = self.table_grid_left_content(anchor_row);
            let grid_top = self.line_map.y_range(anchor_row..anchor_row + 1).start
                + self.line_map.line_text_height(anchor_row)
                + px(super::table::GRID_TOP_GAP);
            let grid_local = point(local.x - grid_left, local.y - grid_top);
            let Some(hit) = table_grid_hit(layout, grid_local) else {
                continue;
            };
            let Some(row) = self.closest_cell_row_for_grid_hit(anchor_row, hit.r, hit.c, local.y)
            else {
                continue;
            };

            let col = if let Some(slot) = self.cell_slots.get(&row) {
                let base_x = slot.text_left + self.row_layout_x(row);
                self.line_map
                    .closest_col(row, point(local.x - base_x, local.y - slot.top))
            } else {
                let cell_left = grid_left + layout.col_x.get(hit.c).copied().unwrap_or(px(0.0));
                let cell_width = layout
                    .col_w
                    .get(hit.c)
                    .copied()
                    .unwrap_or(px(super::table::MIN_COL_W));
                if local.x < cell_left + cell_width / 2.0 {
                    0
                } else {
                    self.line_len(row)
                }
            };

            return Some(self.clamp_location(TextLocation { row, col }));
        }

        None
    }

    fn closest_cell_row_for_grid_hit(
        &self,
        anchor: usize,
        table_row: usize,
        col: usize,
        local_y: Pixels,
    ) -> Option<usize> {
        let mut best: Option<(Pixels, usize)> = None;
        for (row, editor_row) in self.rows.iter().enumerate() {
            let path = editor_row.path;
            if !path.is_cell() || path.anchor != anchor || path.r != table_row || path.c != col {
                continue;
            }

            let Some(slot) = self.cell_slots.get(&row) else {
                return Some(row);
            };
            let annotation_height = self
                .line_map
                .item_line(row)
                .and_then(|line| line.annotation.as_ref())
                .map(|annotation| annotation.height + px(ANNOTATION_BAR_GAP))
                .unwrap_or(px(0.0));
            let row_top = slot.top;
            let row_bottom = row_top
                + self
                    .line_map
                    .line_text_height(row)
                    .max(self.line_map.row_line_height(row))
                + annotation_height;
            let score = if local_y < row_top {
                row_top - local_y
            } else if local_y > row_bottom {
                local_y - row_bottom
            } else {
                px(0.0)
            };
            if best.as_ref().is_none_or(|(current, _)| score < *current) {
                best = Some((score, row));
            }
        }

        best.map(|(_, row)| row)
    }

    /// A click in the gutter immediately left or right of a table's grid (at the
    /// table's vertical level) places the caret right before the table (left) or
    /// right after it (right), instead of falling through to a cell or the caret
    /// at the end of the document.
    fn table_gutter_location_for_local_point(&self, local: Point<Pixels>) -> Option<TextLocation> {
        for (&anchor_row, layout) in &self.table_layouts {
            let Some(object) = self.last_block_object_range_for_row(anchor_row) else {
                continue;
            };
            let band = self.line_map.y_range(anchor_row..anchor_row + 1);
            if local.y < band.start || local.y >= band.end {
                continue;
            }
            let grid_left = self.table_grid_left_content(anchor_row);
            if local.x < grid_left {
                return Some(self.clamp_location(TextLocation {
                    row: anchor_row,
                    col: object.start,
                }));
            }
            if local.x >= grid_left + layout.grid_w {
                return Some(self.clamp_location(TextLocation {
                    row: anchor_row,
                    col: object.end,
                }));
            }
        }
        None
    }

    /// The caret position immediately after the document's trailing block
    /// (image, or table — walking back over the table's in-grid cell rows to the
    /// block's object row). `None` unless the very last line is a block, so a
    /// trailing text line keeps the normal end-of-document behavior.
    fn after_trailing_block_location(&self) -> Option<TextLocation> {
        let last = self.line_map.line_count().checked_sub(1)?;
        for row in (0..=last).rev() {
            if let Some(object) = self.last_block_object_range_for_row(row) {
                return Some(self.clamp_location(TextLocation {
                    row,
                    col: object.end,
                }));
            }
            // Only keep walking back through the trailing table's own cell rows;
            // a non-cell, non-block row means the last line isn't a block.
            if !self.rows.get(row).is_some_and(|editor_row| editor_row.path.is_cell()) {
                break;
            }
        }
        None
    }

    fn block_suffix_location_for_local_point(&self, local: Point<Pixels>) -> Option<TextLocation> {
        for row in 0..self.rows.len() {
            let Some(object) = self.last_block_object_range_for_row(row) else {
                continue;
            };
            let Some(item_line) = self.line_map.item_line(row) else {
                continue;
            };
            let Some(suffix) = item_line.block_suffix.as_ref() else {
                continue;
            };
            let Some(origin) = self.block_suffix_origin_content(row, item_line) else {
                continue;
            };
            let height = suffix.size(self.line_map.row_line_height(row)).height;
            if local.y < origin.y || local.y >= origin.y + height {
                continue;
            }
            let local_point = point(local.x - origin.x, local.y - origin.y);
            let suffix_col = match suffix
                .closest_index_for_position(local_point, self.line_map.row_line_height(row))
            {
                Ok(col) | Err(col) => col,
            };
            return Some(self.clamp_location(TextLocation {
                row,
                col: object.end + suffix_col,
            }));
        }
        None
    }

    pub(super) fn last_block_object_range_for_row(&self, row: usize) -> Option<Range<usize>> {
        let line = self
            .line_range(row)
            .and_then(|range| self.text.get(range))?;
        block_object_ranges(line).into_iter().last()
    }

    pub(super) fn block_suffix_origin_content(
        &self,
        row: usize,
        item_line: &SchemeItemLine,
    ) -> Option<Point<Pixels>> {
        if let Some(layout) = self.table_layouts.get(&row) {
            let line_top = self.line_map.y_range(row..row + 1).start;
            return Some(point(
                self.table_grid_left_content(row),
                line_top
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
            base_x + self.first_text_x(row),
            base_y
                + self.line_map.line_text_height(row)
                + annotation_height
                + media_height
                + item_line.block_suffix_gap,
        ))
    }

    pub(super) fn paint_selection(&self, text_origin: Point<Pixels>, window: &mut Window) {
        let (start, end) = self.selection.ordered();
        let selection_bg = text_selection_rgba(self.theme);
        for row in start.row..=end.row {
            if row >= self.line_map.line_count() {
                continue;
            }
            let (base_x, base_y) = self.row_base_xy(row);
            let line_len = self.line_map.line_len(row);
            let selection_start = (if row == start.row { start.col } else { 0 }).min(line_len);
            let selection_end = (if row == end.row { end.col } else { line_len }).min(line_len);
            if selection_start == selection_end {
                if self.line_map.line(row).is_some() {
                    let x = self
                        .line_map
                        .position_for_index(row, selection_start)
                        .map(|pos| pos.x)
                        .unwrap_or(px(0.0));
                    window.paint_quad(fill(
                        Bounds::new(
                            point(text_origin.x + base_x + x, text_origin.y + base_y),
                            size(
                                px(EMPTY_SELECTION_WIDTH),
                                self.line_map.row_line_height(row),
                            ),
                        ),
                        selection_bg,
                    ));
                }
                continue;
            }

            if self.line_map.line(row).is_none() {
                continue;
            }
            let line_y = base_y;
            let lh = self.line_map.row_line_height(row);
            for (wrap_ix, wrap_range) in self
                .line_map
                .wrapped_line_ranges(row)
                .into_iter()
                .enumerate()
            {
                let start = selection_start.max(wrap_range.start);
                let end = selection_end.min(wrap_range.end);
                if start >= end {
                    continue;
                }
                // position_for_index(i) uses the ix=0 branch when i == wrap_boundary,
                // which attributes the boundary char to the first visual line and returns
                // a large unwrapped x. For non-first wrap lines whose start == wrap_range.start,
                // the first visible character is always at x=0 (left edge).
                let x1 = if wrap_ix > 0 && start == wrap_range.start {
                    px(0.0)
                } else {
                    self.line_map
                        .position_for_index(row, start)
                        .map(|p| p.x)
                        .unwrap_or(px(0.0))
                };
                let x2 = self
                    .line_map
                    .position_for_index(row, end)
                    .map(|p| p.x)
                    .unwrap_or(x1 + px(5.0));
                let y = line_y + lh * wrap_ix as f32;
                window.paint_quad(fill(
                    Bounds::new(
                        point(text_origin.x + base_x + x1, text_origin.y + y),
                        size((x2 - x1).max(px(4.0)), lh),
                    ),
                    selection_bg,
                ));
            }
        }
    }

    pub(super) fn paint_block_object_selection(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let (start, end) = self.selection.ordered();
        let selection_bg = text_selection_rgba(self.theme);
        for row in start.row..=end.row {
            if row >= self.line_map.line_count()
                || self.rows.get(row).is_some_and(|row| row.path.is_cell())
            {
                continue;
            }
            let line_len = self.line_map.line_len(row);
            let selection_start = (if row == start.row { start.col } else { 0 }).min(line_len);
            let selection_end = (if row == end.row { end.col } else { line_len }).min(line_len);
            if selection_start >= selection_end {
                continue;
            }
            let Some(line) = self.line_range(row).and_then(|range| self.text.get(range)) else {
                continue;
            };
            for object in block_object_ranges(line) {
                if selection_start < object.end && object.start < selection_end {
                    if let Some(block_bounds) =
                        self.block_object_selection_bounds(row, object.start, bounds)
                    {
                        window.paint_quad(fill(block_bounds, selection_bg));
                    }
                }
            }
        }
    }

    fn block_object_selection_bounds(
        &self,
        row: usize,
        object_start: usize,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        if self
            .table_object_range_for_row(row)
            .is_some_and(|object| object.start == object_start)
        {
            let (origin, grid_w, grid_h) = self.table_grid_geom(row, bounds)?;
            return Some(Bounds::new(origin, size(grid_w, grid_h)));
        }

        let (_, image_index) = self.image_object_at_location(TextLocation {
            row,
            col: object_start,
        })?;
        let image = self.image_bounds_for_index_content(row, image_index)?;
        Some(Bounds::new(
            self.content_to_window(image.origin, bounds),
            image.size,
        ))
    }

    pub(super) fn location_for_window_position(&self, position: Point<Pixels>) -> TextLocation {
        let Some(bounds) = self.last_bounds else {
            return self.selection.head;
        };
        let text_origin = point(
            bounds.left() + px(TEXT_LEFT_PAD),
            bounds.top() + px(self.top_pad),
        );
        self.location_for_local_point(point(
            position.x - text_origin.x,
            position.y - text_origin.y,
        ))
    }
}

pub(super) fn bounds_contains(bounds: Bounds<Pixels>, point: Point<Pixels>) -> bool {
    point.x >= bounds.left()
        && point.x <= bounds.right()
        && point.y >= bounds.top()
        && point.y <= bounds.bottom()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TableGridHit {
    r: usize,
    c: usize,
}

fn table_grid_hit(
    layout: &super::table::TableLayout,
    local: Point<Pixels>,
) -> Option<TableGridHit> {
    if local.x < px(0.0)
        || local.y < px(0.0)
        || local.x > layout.grid_w
        || local.y > layout.grid_h() + px(super::table::BOTTOM_HIT_SLACK)
        || layout.col_w.is_empty()
    {
        return None;
    }

    let last_col = layout.col_w.len().saturating_sub(1);
    let c = layout.col_w.iter().enumerate().find_map(|(c, width)| {
        let right = layout.col_x.get(c).copied().unwrap_or(px(0.0)) + *width;
        (local.x < right || c == last_col).then_some(c)
    })?;

    let r = if local.y < layout.header_h {
        HEADER_ROW
    } else {
        let body_y = local.y - layout.header_h;
        let last_row = layout.body_band_h.len().saturating_sub(1);
        let mut top = px(0.0);
        layout
            .body_band_h
            .iter()
            .enumerate()
            .find_map(|(r, height)| {
                let bottom = top + *height;
                let is_hit = body_y < bottom || r == last_row;
                top = bottom;
                is_hit.then_some(r)
            })?
    };

    Some(TableGridHit { r, c })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_layout_for_hit_tests() -> super::super::table::TableLayout {
        super::super::table::TableLayout {
            col_x: vec![px(0.0), px(100.0)],
            col_w: vec![px(100.0), px(160.0)],
            grid_w: px(260.0),
            header_h: px(30.0),
            body_band_h: vec![px(40.0), px(50.0)],
            block_height: px(0.0),
        }
    }

    #[test]
    fn table_grid_hit_maps_padding_and_empty_cell_space_to_cells() {
        let layout = table_layout_for_hit_tests();

        assert_eq!(
            table_grid_hit(&layout, point(px(8.0), px(8.0))),
            Some(TableGridHit {
                r: HEADER_ROW,
                c: 0,
            })
        );
        assert_eq!(
            table_grid_hit(&layout, point(px(130.0), px(38.0))),
            Some(TableGridHit { r: 0, c: 1 })
        );
        assert_eq!(
            table_grid_hit(&layout, point(px(259.0), px(119.0))),
            Some(TableGridHit { r: 1, c: 1 })
        );
    }

    #[test]
    fn table_grid_hit_maps_bottom_slack_to_the_last_row() {
        let layout = table_layout_for_hit_tests();

        // Just below the last row (the add-row control strip) reads as the last
        // row of the clicked column rather than missing the table.
        assert_eq!(
            table_grid_hit(&layout, point(px(8.0), px(121.0))),
            Some(TableGridHit { r: 1, c: 0 })
        );
    }

    #[test]
    fn table_grid_hit_rejects_points_outside_the_frame() {
        let layout = table_layout_for_hit_tests();

        assert_eq!(table_grid_hit(&layout, point(px(-1.0), px(8.0))), None);
        assert_eq!(table_grid_hit(&layout, point(px(8.0), px(-1.0))), None);
        // Past the right edge (the side gutters route before/after the table, not
        // into a cell), and past the bottom slack.
        assert_eq!(table_grid_hit(&layout, point(px(261.0), px(8.0))), None);
        assert_eq!(table_grid_hit(&layout, point(px(8.0), px(140.0))), None);
    }
}
