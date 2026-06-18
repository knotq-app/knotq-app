use gpui::{fill, point, px, size, Bounds, Pixels, Point, Window};

use crate::line_map::TextLocation;
use crate::theme_gpui::text_selection_rgba;

use super::{
    SchemeEditor, CHECKBOX_GAP, CHECKBOX_SIZE, EMPTY_SELECTION_WIDTH, INDENT_WIDTH, MAX_INDENT,
    TEXT_LEFT_PAD,
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
                if let Some(point) = self.table_suffix_point_for_location(loc, object.end) {
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
        let intra = self
            .line_map
            .position_for_index(loc.row, loc.col)
            .unwrap_or_else(|| point(px(0.0), px(0.0)));
        let (base_x, base_y) = self.row_base_xy(loc.row);
        point(base_x + intra.x, base_y + intra.y)
    }

    fn table_suffix_point_for_location(
        &self,
        loc: TextLocation,
        suffix_start: usize,
    ) -> Option<Point<Pixels>> {
        let item_line = self.line_map.item_line(loc.row)?;
        let suffix = item_line.block_suffix.as_ref()?;
        let layout = self.table_layouts.get(&loc.row)?;
        let suffix_col = loc.col.saturating_sub(suffix_start);
        let suffix_pos = suffix
            .position_for_index(suffix_col, self.line_map.row_line_height(loc.row))
            .unwrap_or_else(|| point(px(0.0), px(0.0)));
        let line_top = self.line_map.y_range(loc.row..loc.row + 1).start;
        Some(point(
            self.table_grid_left_content(loc.row) + suffix_pos.x,
            line_top
                + self.line_map.line_text_height(loc.row)
                + layout.block_height
                + item_line.block_suffix_gap
                + suffix_pos.y,
        ))
    }

    pub(super) fn location_for_local_point(&self, local: Point<Pixels>) -> TextLocation {
        if self.line_map.line_count() == 0 {
            return TextLocation { row: 0, col: 0 };
        }

        if let Some(loc) = self.cell_location_for_local_point(local) {
            return loc;
        }
        if let Some(loc) = self.table_suffix_location_for_local_point(local) {
            return loc;
        }

        let probe = self.line_map.location_for_point(point(px(0.0), local.y));
        let row = probe.row.min(self.line_map.line_count().saturating_sub(1));
        self.clamp_location(
            self.line_map
                .location_for_point(point(local.x - self.row_layout_x(row), local.y)),
        )
    }

    fn cell_location_for_local_point(&self, local: Point<Pixels>) -> Option<TextLocation> {
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

    fn table_suffix_location_for_local_point(&self, local: Point<Pixels>) -> Option<TextLocation> {
        for row in 0..self.rows.len() {
            let Some(object) = self.table_object_range_for_row(row) else {
                continue;
            };
            let Some(item_line) = self.line_map.item_line(row) else {
                continue;
            };
            let Some(suffix) = item_line.block_suffix.as_ref() else {
                continue;
            };
            let Some(layout) = self.table_layouts.get(&row) else {
                continue;
            };
            let left = self.table_grid_left_content(row);
            let top = self.line_map.y_range(row..row + 1).start
                + self.line_map.line_text_height(row)
                + layout.block_height
                + item_line.block_suffix_gap;
            let height = suffix.size(self.line_map.row_line_height(row)).height;
            if local.y < top || local.y >= top + height {
                continue;
            }
            let local_point = point(local.x - left, local.y - top);
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
