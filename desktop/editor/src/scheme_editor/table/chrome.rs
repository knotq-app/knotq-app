use gpui::Corners;

use super::super::*;
use super::*;

impl SchemeEditor {
    pub(in crate::scheme_editor) fn table_control_at(&self, position: Point<Pixels>) -> Option<TableControlHitbox> {
        self.table_control_hitboxes
            .iter()
            .copied()
            .find(|hitbox| bounds_contains(hitbox.bounds, position))
    }

    pub(in crate::scheme_editor) fn table_context_at_position(
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

    pub(in crate::scheme_editor) fn paint_table_chrome(
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
