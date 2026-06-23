use std::collections::HashMap;

use gpui::{Hsla, WrappedLine};

use super::super::*;
use super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn table_header_text_color(&self) -> Hsla {
        // `text_soft` (vs the fainter `text_muted`) keeps column headers legible
        // in light themes; the semibold weight applied in layout does the rest.
        token_hsla(self.theme.text_soft)
    }

    pub(in crate::scheme_editor) fn table_grid_left_content(&self, anchor_row: usize) -> Pixels {
        grid_left_content_for_indent(self.row_indent_x(anchor_row))
    }

    pub(in crate::scheme_editor) fn table_content_width(&self, anchor_row: usize, wrap_width: Pixels) -> Pixels {
        table_content_width_for_indent(wrap_width, self.row_indent_x(anchor_row))
    }

    pub(in crate::scheme_editor) fn build_table_layout(
        &self,
        item: &Item,
        content_width: Pixels,
        window: &mut Window,
    ) -> Option<TableLayout> {
        let table = item.table()?;
        let ncols = table.column_count().max(1);
        let lh = px(CELL_LINE_HEIGHT);
        let font = window.text_style().font();

        let even = (content_width / ncols as f32).max(px(MIN_COL_W));
        let mut col_w = Vec::with_capacity(ncols);
        for col in &table.columns {
            col_w.push(col.width.map(px).unwrap_or(even).max(px(MIN_COL_W)));
        }
        while col_w.len() < ncols {
            col_w.push(even);
        }

        let mut col_x = Vec::with_capacity(ncols);
        let mut acc = px(0.0);
        for width in &col_w {
            col_x.push(acc);
            acc += *width;
        }
        let grid_w = acc;

        let header_color = self.table_header_text_color();
        let mut header_h = px(ROW_MIN_H);
        for (c, col) in table.columns.iter().enumerate() {
            let text_w = (col_w[c] - px(CELL_PAD_X * 2.0)).max(px(16.0));
            let line = self.shape_cell_line(&font, &col.name, text_w, header_color, true, window);
            header_h = header_h.max(line.size(lh).height + px(CELL_PAD_Y * 2.0));
        }

        let body_color = token_hsla(self.theme.text_primary);
        let mut body_band_h = Vec::with_capacity(table.row_count());
        for table_row in &table.rows {
            let mut band = px(ROW_MIN_H);
            for (c, cell) in table_row.cells.iter().enumerate() {
                let text_w = (col_w[c] - px(CELL_PAD_X * 2.0)).max(px(16.0));
                let mut cell_h = px(0.0);
                for sub in &cell.items {
                    let display = clean_line_text(&sub.text());
                    let line =
                        self.shape_cell_line(&font, &display, text_w, body_color, false, window);
                    cell_h += line.size(lh).height.max(lh);
                    if annotation_text(sub, self.time_format).is_some() {
                        cell_h += px(ANNOTATION_HEIGHT) + px(ANNOTATION_BAR_GAP);
                    }
                }
                band = band.max(cell_h + px(CELL_PAD_Y * 2.0));
            }
            body_band_h.push(band);
        }

        let grid_h = header_h + body_band_h.iter().fold(px(0.0), |acc, h| acc + *h);
        let block_height = px(GRID_TOP_GAP) + grid_h + px(CONTROL_H) + px(GRID_BOTTOM_GAP);

        Some(TableLayout {
            col_x,
            col_w,
            grid_w,
            header_h,
            body_band_h,
            block_height,
        })
    }

    pub(in crate::scheme_editor) fn shape_cell_line(
        &self,
        font: &gpui::Font,
        text: &str,
        width: Pixels,
        color: Hsla,
        bold: bool,
        window: &mut Window,
    ) -> WrappedLine {
        let mut font = font.clone();
        font.family = SharedString::new(FONT_UI);
        if bold {
            font.weight = gpui::FontWeight::SEMIBOLD;
        }
        let run = TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_text(
                SharedString::new(text.to_string()),
                px(TEXT_FONT_SIZE),
                &[run],
                Some(width),
                None,
            )
            .unwrap_or_default()
            .pop()
            .unwrap_or_default()
    }

    pub(in crate::scheme_editor) fn compute_cell_slots(&mut self) {
        self.cell_slots.clear();
        let mut i = 0;
        while i < self.rows.len() {
            if !self.rows[i].path.is_table_anchor() {
                i += 1;
                continue;
            }
            let anchor = i;
            let Some(layout) = self.table_layouts.get(&anchor).cloned() else {
                i += 1;
                continue;
            };
            let grid_left = self.table_grid_left_content(anchor);
            let anchor_top = self.line_map.y_range(anchor..anchor + 1).start;
            let header_top = anchor_top + self.line_map.line_text_height(anchor) + px(GRID_TOP_GAP);
            let body_top = header_top + layout.header_h;

            let mut band_top = Vec::with_capacity(layout.body_band_h.len());
            let mut acc = body_top;
            for height in &layout.body_band_h {
                band_top.push(acc);
                acc += *height;
            }

            let mut cell_run: HashMap<(usize, usize), Pixels> = HashMap::new();
            i += 1;
            while i < self.rows.len() && self.rows[i].path.is_cell() {
                let path = self.rows[i].path;
                let col_left = grid_left + layout.col_x.get(path.c).copied().unwrap_or(px(0.0));
                let text_left = col_left + px(CELL_PAD_X) + px(CHECKBOX_SIZE + CHECKBOX_GAP);
                let width = (layout.col_w.get(path.c).copied().unwrap_or(px(MIN_COL_W))
                    - px(CELL_PAD_X * 2.0))
                .max(px(16.0));
                let annotation = self
                    .line_map
                    .item_line(i)
                    .and_then(|line| line.annotation.as_ref())
                    .map(|annotation| annotation.height)
                    .unwrap_or(px(0.0));
                let top = if path.is_header_cell() {
                    // Header cells sit in the header band, vertically centered.
                    let text_h = self.line_map.line_text_height(i).max(px(CELL_LINE_HEIGHT));
                    header_top + ((layout.header_h - text_h) / 2.0).max(px(0.0))
                } else {
                    let cell_base =
                        band_top.get(path.r).copied().unwrap_or(body_top) + px(CELL_PAD_Y);
                    let run = cell_run.entry((path.r, path.c)).or_insert(px(0.0));
                    let top = cell_base + *run;
                    *run +=
                        self.line_map.line_text_height(i).max(px(CELL_LINE_HEIGHT)) + annotation;
                    top
                };
                self.cell_slots.insert(
                    i,
                    CellSlot {
                        text_left,
                        col_left,
                        top,
                        width,
                    },
                );
                i += 1;
            }
        }
    }

    pub(in crate::scheme_editor) fn table_grid_geom(
        &self,
        anchor_row: usize,
        bounds: Bounds<Pixels>,
    ) -> Option<(Point<Pixels>, Pixels, Pixels)> {
        let layout = self.table_layouts.get(&anchor_row)?;
        let origin = self.content_to_window(
            point(
                self.table_grid_left_content(anchor_row),
                self.line_map.y_range(anchor_row..anchor_row + 1).start
                    + self.line_map.line_text_height(anchor_row)
                    + px(GRID_TOP_GAP),
            ),
            bounds,
        );
        Some((origin, layout.grid_w, layout.grid_h()))
    }

    pub(in crate::scheme_editor) fn content_to_window(
        &self,
        content: Point<Pixels>,
        bounds: Bounds<Pixels>,
    ) -> Point<Pixels> {
        point(
            bounds.left() + px(TEXT_LEFT_PAD) + content.x,
            bounds.top() + px(self.top_pad) + content.y,
        )
    }
}
