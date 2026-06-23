use std::ops::Range;

use gpui::{point, px, size, Bounds, Pixels, Point};

use crate::line_map::{SchemeItemLine, TextLocation};

use super::super::{
    block_object_ranges, clean_line_text, SchemeEditor, CHECKBOX_GAP, CHECKBOX_SIZE, INDENT_WIDTH,
    MAX_INDENT, TEXT_LEFT_PAD,
};

impl SchemeEditor {
    pub(in crate::scheme_editor) fn row_bounds(&self, row: usize, bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
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

    pub(in crate::scheme_editor) fn row_indent_x(&self, row: usize) -> Pixels {
        px(self
            .rows
            .get(row)
            .map(|row| row.item.indent.min(MAX_INDENT) as f32)
            .unwrap_or(0.0)
            * INDENT_WIDTH)
    }

    pub(in crate::scheme_editor) fn row_layout_x(&self, row: usize) -> Pixels {
        self.row_indent_x(row) - self.row_plain_text_shift_x(row) - self.first_text_x(row)
    }

    pub(in crate::scheme_editor) fn row_plain_text_shift_x(&self, row: usize) -> Pixels {
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

    pub(in crate::scheme_editor) fn first_text_x(&self, row: usize) -> Pixels {
        self.line_map
            .position_for_index(row, 0)
            .map(|point| point.x)
            .unwrap_or(px(0.0))
    }

    /// Content-space (relative to the text origin) base offset to add to a
    /// line's intra-line position. Cell rows live inside their table slot;
    /// ordinary rows keep the vertical flow position from the line map.
    pub(in crate::scheme_editor) fn row_base_xy(&self, row: usize) -> (Pixels, Pixels) {
        if let Some(slot) = self.cell_slots.get(&row) {
            (slot.text_left + self.row_layout_x(row), slot.top)
        } else {
            (
                self.row_layout_x(row),
                self.line_map.y_range(row..row + 1).start,
            )
        }
    }

    pub(in crate::scheme_editor) fn visual_point_for_location(&self, loc: TextLocation) -> Point<Pixels> {
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
                        + px(crate::scheme_editor::table::GRID_TOP_GAP);
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

    pub(in crate::scheme_editor) fn last_block_object_range_for_row(&self, row: usize) -> Option<Range<usize>> {
        let line = self
            .line_range(row)
            .and_then(|range| self.text.get(range))?;
        block_object_ranges(line).into_iter().last()
    }

    pub(in crate::scheme_editor) fn block_suffix_origin_content(
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

    pub(in crate::scheme_editor) fn location_for_window_position(&self, position: Point<Pixels>) -> TextLocation {
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
