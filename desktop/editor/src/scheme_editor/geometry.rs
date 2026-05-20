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

    pub(super) fn visual_point_for_location(&self, loc: TextLocation) -> Point<Pixels> {
        let loc = self.clamp_location(loc);
        let mut point = self.line_map.point_for_location(loc);
        point.x += self.row_layout_x(loc.row);
        point
    }

    pub(super) fn location_for_local_point(&self, local: Point<Pixels>) -> TextLocation {
        if self.line_map.line_count() == 0 {
            return TextLocation { row: 0, col: 0 };
        }

        let probe = self.line_map.location_for_point(point(px(0.0), local.y));
        let row = probe.row.min(self.line_map.line_count().saturating_sub(1));
        self.clamp_location(
            self.line_map
                .location_for_point(point(local.x - self.row_layout_x(row), local.y)),
        )
    }

    pub(super) fn paint_selection(&self, text_origin: Point<Pixels>, window: &mut Window) {
        let (start, end) = self.selection.ordered();
        let selection_bg = text_selection_rgba(self.theme);
        for row in start.row..=end.row {
            if row >= self.line_map.line_count() {
                continue;
            }
            let indent_x = self.row_layout_x(row);
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
                    let y = self.line_map.y_range(row..row + 1).start;
                    window.paint_quad(fill(
                        Bounds::new(
                            point(text_origin.x + indent_x + x, text_origin.y + y),
                            size(px(EMPTY_SELECTION_WIDTH), self.line_map.line_height()),
                        ),
                        selection_bg,
                    ));
                }
                continue;
            }

            if self.line_map.line(row).is_none() {
                continue;
            }
            let line_y = self.line_map.y_range(row..row + 1).start;
            let lh = self.line_map.line_height();
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
                        point(text_origin.x + indent_x + x1, text_origin.y + y),
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
