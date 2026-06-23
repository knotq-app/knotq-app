use gpui::{fill, point, px, size, Bounds, Pixels, Point, Window};

use crate::line_map::TextLocation;
use crate::theme_gpui::text_selection_rgba;

use super::super::{block_object_ranges, SchemeEditor, EMPTY_SELECTION_WIDTH};

impl SchemeEditor {
    pub(in crate::scheme_editor) fn paint_selection(&self, text_origin: Point<Pixels>, window: &mut Window) {
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

    pub(in crate::scheme_editor) fn paint_block_object_selection(&self, bounds: Bounds<Pixels>, window: &mut Window) {
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
}
