use gpui::{point, px, Pixels, Point};

use crate::line_map::TextLocation;

use super::super::{buffer::HEADER_ROW, SchemeEditor, ANNOTATION_BAR_GAP};

impl SchemeEditor {
    pub(in crate::scheme_editor) fn location_for_local_point(&self, local: Point<Pixels>) -> TextLocation {
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
        let pad = px(crate::scheme_editor::table::CELL_PAD_X);
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
                + px(crate::scheme_editor::table::GRID_TOP_GAP);
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
                    .unwrap_or(px(crate::scheme_editor::table::MIN_COL_W));
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TableGridHit {
    r: usize,
    c: usize,
}

fn table_grid_hit(
    layout: &crate::scheme_editor::table::TableLayout,
    local: Point<Pixels>,
) -> Option<TableGridHit> {
    if local.x < px(0.0)
        || local.y < px(0.0)
        || local.x > layout.grid_w
        || local.y > layout.grid_h() + px(crate::scheme_editor::table::BOTTOM_HIT_SLACK)
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

    fn table_layout_for_hit_tests() -> crate::scheme_editor::table::TableLayout {
        crate::scheme_editor::table::TableLayout {
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
