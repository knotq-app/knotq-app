//! Table grid geometry and chrome.
//!
//! A table is flattened into the editor's one text buffer: an anchor row
//! reserves vertical space and owns the grid chrome, while each cell line is a
//! real editable row positioned inside the grid via a [`CellSlot`].

use std::collections::HashMap;

use gpui::{Corners, Hsla, WrappedLine};
use knotq_model::Table;

use super::*;

pub(super) const MIN_COL_W: f32 = 132.0;
pub(super) const CELL_PAD_X: f32 = 10.0;
pub(super) const CELL_PAD_Y: f32 = 6.0;
pub(super) const GRID_TOP_GAP: f32 = 6.0;
const ROW_MIN_H: f32 = 30.0;
const GRID_BOTTOM_GAP: f32 = 2.0;
const CONTROL_H: f32 = 16.0;
const CONTROL_BTN: f32 = 14.0;
const RIGHT_MARGIN: f32 = CONTROL_BTN + 10.0;
const CELL_LINE_HEIGHT: f32 = TEXT_LINE_HEIGHT;

pub(super) fn grid_left_content() -> Pixels {
    px(-(CHECKBOX_SIZE + CHECKBOX_GAP))
}

fn grid_left_content_for_indent(indent_x: Pixels) -> Pixels {
    grid_left_content() + indent_x
}

fn table_content_width_for_indent(wrap_width: Pixels, indent_x: Pixels) -> Pixels {
    (wrap_width - px(TEXT_LEFT_PAD) - indent_x + px(CHECKBOX_SIZE + CHECKBOX_GAP)
        - px(RIGHT_MARGIN))
    .max(px(MIN_COL_W))
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CellSlot {
    pub(super) text_left: Pixels,
    pub(super) col_left: Pixels,
    pub(super) top: Pixels,
    pub(super) width: Pixels,
}

#[derive(Clone)]
pub(super) struct TableLayout {
    pub(super) col_x: Vec<Pixels>,
    pub(super) col_w: Vec<Pixels>,
    pub(super) grid_w: Pixels,
    pub(super) header_h: Pixels,
    pub(super) body_band_h: Vec<Pixels>,
    pub(super) block_height: Pixels,
}

impl TableLayout {
    fn grid_h(&self) -> Pixels {
        self.header_h + self.body_band_h.iter().fold(px(0.0), |acc, h| acc + *h)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TableControlKind {
    AddRow,
    AddColumn,
}

#[derive(Clone, Copy)]
pub(super) struct TableControlHitbox {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) anchor_row: usize,
    pub(super) kind: TableControlKind,
}

impl SchemeEditor {
    pub(super) fn table_header_text_color(&self) -> Hsla {
        // `text_soft` (vs the fainter `text_muted`) keeps column headers legible
        // in light themes; the semibold weight applied in layout does the rest.
        token_hsla(self.theme.text_soft)
    }

    pub(super) fn table_grid_left_content(&self, anchor_row: usize) -> Pixels {
        grid_left_content_for_indent(self.row_indent_x(anchor_row))
    }

    pub(super) fn table_content_width(&self, anchor_row: usize, wrap_width: Pixels) -> Pixels {
        table_content_width_for_indent(wrap_width, self.row_indent_x(anchor_row))
    }

    pub(super) fn build_table_layout(
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

    pub(super) fn shape_cell_line(
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

    pub(super) fn compute_cell_slots(&mut self) {
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

    pub(super) fn table_grid_geom(
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

    pub(super) fn content_to_window(
        &self,
        content: Point<Pixels>,
        bounds: Bounds<Pixels>,
    ) -> Point<Pixels> {
        point(
            bounds.left() + px(TEXT_LEFT_PAD) + content.x,
            bounds.top() + px(self.top_pad) + content.y,
        )
    }

    pub(super) fn table_control_at(&self, position: Point<Pixels>) -> Option<TableControlHitbox> {
        self.table_control_hitboxes
            .iter()
            .copied()
            .find(|hitbox| bounds_contains(hitbox.bounds, position))
    }

    pub(super) fn table_context_at_position(
        &self,
        position: Point<Pixels>,
    ) -> Option<TableContext> {
        let bounds = self.last_bounds?;
        for anchor_row in 0..self.rows.len() {
            let anchor = self.rows.get(anchor_row)?;
            if !anchor.path.is_table_anchor() {
                continue;
            }
            let table = anchor.item.table()?;
            let layout = self.table_layouts.get(&anchor_row)?;
            let (origin, grid_w, grid_h) = self.table_grid_geom(anchor_row, bounds)?;
            if position.x < origin.x
                || position.x >= origin.x + grid_w
                || position.y < origin.y
                || position.y >= origin.y + grid_h
            {
                continue;
            }

            let local_x = position.x - origin.x;
            let local_y = position.y - origin.y;
            let column = layout.col_x.iter().zip(&layout.col_w).enumerate().find_map(
                |(c, (left, width))| (local_x >= *left && local_x < *left + *width).then_some(c),
            );
            let row = if local_y < layout.header_h {
                None
            } else {
                let body_y = local_y - layout.header_h;
                let mut top = px(0.0);
                layout
                    .body_band_h
                    .iter()
                    .enumerate()
                    .find_map(|(r, height)| {
                        let in_row = body_y >= top && body_y < top + *height;
                        top += *height;
                        in_row.then_some(r)
                    })
            };

            return Some(TableContext {
                table_item_id: anchor.item.id,
                row,
                column,
                row_count: table.row_count(),
                column_count: table.column_count(),
            });
        }
        None
    }

    pub(super) fn paint_table_chrome(
        &mut self,
        anchor_row: usize,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(layout) = self.table_layouts.get(&anchor_row).cloned() else {
            return;
        };
        let Some((origin, grid_w, grid_h)) = self.table_grid_geom(anchor_row, bounds) else {
            return;
        };
        let theme = self.theme;
        let border = token_hsla(theme.border_soft);
        let header_bg = token_rgba(theme.row_hover);

        let frame = Bounds::new(origin, size(grid_w, grid_h));
        window.paint_quad(quad(
            frame,
            px(7.0),
            token_rgba(theme.bg_app),
            px(1.0),
            border,
            BorderStyle::default(),
        ));
        // Inset the header fill inside the frame's 1px border and round only its
        // top corners so it follows the frame's rounded top instead of leaving
        // square nubs over the corners.
        let inset = px(1.0);
        window.paint_quad(quad(
            Bounds::new(
                point(origin.x + inset, origin.y + inset),
                size(grid_w - inset * 2.0, layout.header_h - inset),
            ),
            Corners {
                top_left: px(6.0),
                top_right: px(6.0),
                bottom_left: px(0.0),
                bottom_right: px(0.0),
            },
            header_bg,
            px(0.0),
            border,
            BorderStyle::default(),
        ));

        for c in 1..layout.col_w.len() {
            let x = origin.x + layout.col_x[c];
            window.paint_quad(fill(
                Bounds::new(point(x, origin.y), size(px(1.0), grid_h)),
                border,
            ));
        }

        let mut y = origin.y + layout.header_h;
        window.paint_quad(fill(
            Bounds::new(point(origin.x, y), size(grid_w, px(1.0))),
            token_hsla(theme.border_main),
        ));
        // Separators sit *between* body rows; the final row's bottom edge is the
        // frame border itself, so drawing it here would double the line and poke
        // past the rounded bottom corners.
        let last_band = layout.body_band_h.len().saturating_sub(1);
        for (i, height) in layout.body_band_h.iter().enumerate() {
            y += *height;
            if i < last_band {
                window.paint_quad(fill(
                    Bounds::new(point(origin.x, y), size(grid_w, px(1.0))),
                    border,
                ));
            }
        }

        self.paint_table_controls(anchor_row, origin, grid_w, grid_h, &layout, window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_table_controls(
        &mut self,
        anchor_row: usize,
        origin: Point<Pixels>,
        grid_w: Pixels,
        grid_h: Pixels,
        layout: &TableLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let btn = px(CONTROL_BTN);
        let add_row = Bounds::new(
            point(
                origin.x + (grid_w - btn) / 2.0,
                origin.y + grid_h + px(GRID_BOTTOM_GAP),
            ),
            size(btn, btn),
        );
        let add_row_kind = TableControlKind::AddRow;
        self.paint_control_button(
            add_row,
            "+",
            self.hovered_table_control == Some(add_row_kind),
            window,
            cx,
        );
        self.table_control_hitboxes.push(TableControlHitbox {
            bounds: add_row,
            anchor_row,
            kind: add_row_kind,
        });

        let add_col = Bounds::new(
            point(
                origin.x + grid_w + px(4.0),
                origin.y + (layout.header_h - btn) / 2.0,
            ),
            size(btn, btn),
        );
        let add_col_kind = TableControlKind::AddColumn;
        self.paint_control_button(
            add_col,
            "+",
            self.hovered_table_control == Some(add_col_kind),
            window,
            cx,
        );
        self.table_control_hitboxes.push(TableControlHitbox {
            bounds: add_col,
            anchor_row,
            kind: add_col_kind,
        });
    }

    pub(super) fn insert_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let mut top = reconstruct_top_level(&self.rows);
        let insert_pos = (self.current_top_level_index() + 1).min(top.len());
        let mut table_item = Item::new("");
        table_item.set_table(Table::new(2, 2));
        let table_id = table_item.id;
        top.insert(insert_pos, table_item.clone());

        let (text, rows) = build_buffer(&top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        if let Some(anchor) = self
            .rows
            .iter()
            .position(|row| row.path.is_table_anchor() && row.item.id == table_id)
        {
            if anchor + 1 < self.rows.len() && self.rows[anchor + 1].path.is_cell() {
                self.selection = TextSelection::collapsed(TextLocation {
                    row: anchor + 1,
                    col: 0,
                });
            }
        }
        self.focus(window, cx);
        self.scroll_to_cursor(cx);
        cx.emit(EditorEvent::Command(Command::InsertItem {
            scheme: self.scheme_id,
            position: insert_pos,
            item: table_item,
        }));
        cx.notify();
    }

    fn current_top_level_index(&self) -> usize {
        let row = self.current_row_index();
        let mut index: usize = 0;
        for i in 0..=row.min(self.rows.len().saturating_sub(1)) {
            if !self.rows[i].path.is_cell() {
                index += 1;
            }
        }
        index.saturating_sub(1)
    }

    pub(super) fn apply_table_control(
        &mut self,
        hitbox: TableControlHitbox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let Some(anchor) = self.rows.get(hitbox.anchor_row) else {
            return;
        };
        if !anchor.path.is_table_anchor() {
            return;
        }
        let table_id = anchor.item.id;
        let action = match hitbox.kind {
            TableControlKind::AddRow => TableStructureAction::AppendRow,
            TableControlKind::AddColumn => TableStructureAction::AppendColumn,
        };
        self.apply_table_structure_action(table_id, action, window, cx);
    }

    pub fn apply_table_structure_action(
        &mut self,
        table_id: ItemId,
        action: TableStructureAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let mut top = reconstruct_top_level(&self.rows);
        let Some(pos) = top.iter().position(|item| item.id == table_id) else {
            return;
        };
        let Some(table) = top[pos].table_mut() else {
            return;
        };
        match action {
            TableStructureAction::AppendRow => {
                table.insert_row(table.row_count());
            }
            TableStructureAction::AppendColumn => {
                let n = table.column_count();
                table.insert_column(n, format!("Column {}", n + 1));
            }
            TableStructureAction::InsertRowBefore(row) => {
                table.insert_row(row);
            }
            TableStructureAction::InsertRowAfter(row) => {
                table.insert_row(row.saturating_add(1));
            }
            TableStructureAction::DeleteRow(row) => table.remove_row(row),
            TableStructureAction::InsertColumnBefore(col) => {
                let n = table.column_count();
                table.insert_column(col, format!("Column {}", n + 1));
            }
            TableStructureAction::InsertColumnAfter(col) => {
                let n = table.column_count();
                table.insert_column(col.saturating_add(1), format!("Column {}", n + 1));
            }
            TableStructureAction::DeleteColumn(col) => table.remove_column(col),
        }
        table.normalize();
        let item = top[pos].clone();

        let (text, rows) = build_buffer(&top);
        self.text = text;
        self.rows = rows;
        self.refresh_layout_after_content_change(Some(window));
        self.selection = TextSelection::collapsed(self.clamp_location(self.selection.head));
        self.scroll_to_cursor(cx);
        cx.emit(EditorEvent::Command(Command::ReplaceItem {
            scheme: self.scheme_id,
            item,
        }));
        cx.notify();
    }

    fn paint_control_button(
        &self,
        bounds: Bounds<Pixels>,
        glyph: &str,
        hovered: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = self.theme;
        window.paint_quad(quad(
            bounds,
            px(3.0),
            if hovered {
                token_rgba(theme.button_bg)
            } else {
                token_rgba(0x00000000)
            },
            px(if hovered { 1.0 } else { 0.0 }),
            token_hsla(theme.border_main),
            BorderStyle::default(),
        ));
        let color = if hovered {
            token_hsla(theme.text_primary)
        } else {
            token_hsla(if theme.is_dark {
                0xffffff2a
            } else {
                0x00000026
            })
        };
        let mut font = window.text_style().font().clone();
        font.family = SharedString::new(FONT_UI);
        let run = TextRun {
            len: glyph.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        if let Some(line) = window
            .text_system()
            .shape_text(
                SharedString::new(glyph.to_string()),
                px(13.0),
                &[run],
                None,
                None,
            )
            .unwrap_or_default()
            .pop()
        {
            let glyph = line.size(px(CONTROL_BTN));
            let origin = point(
                bounds.left() + (bounds.size.width - glyph.width) / 2.0,
                bounds.top() + (bounds.size.height - glyph.height) / 2.0,
            );
            let _ = line.paint(origin, px(CONTROL_BTN), TextAlign::Left, None, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indented_table_geometry_shifts_and_shrinks_with_anchor_indent() {
        let wrap_width = px(720.0);
        let indent = px(INDENT_WIDTH * 2.0);

        assert_eq!(
            grid_left_content_for_indent(indent),
            grid_left_content() + indent
        );
        assert_eq!(
            table_content_width_for_indent(wrap_width, indent),
            table_content_width_for_indent(wrap_width, px(0.0)) - indent
        );
    }
}
