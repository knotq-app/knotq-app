use super::super::*;

impl SchemeEditor {
    fn link_under_cursor(&self) -> Option<(usize, Range<usize>, String)> {
        if self.read_only {
            return None;
        }
        let loc = self.selection.head;
        let range = self.line_range(loc.row)?;
        let line = self.text.get(range)?;
        detect_links(line).into_iter().find_map(|link| {
            (loc.col >= link.start && loc.col <= link.end)
                .then(|| (loc.row, link.clone(), link_url(&line[link])))
        })
    }

    /// Paints a small floating "open" button above the link the cursor is in, so
    /// a link can be opened with a plain click without needing Cmd/Ctrl-click.
    /// Records the button bounds in `open_link_button` for hit-testing.
    pub(in crate::scheme_editor) fn paint_link_open_button(
        &mut self,
        bounds: Bounds<Pixels>,
        text_origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((row, link, url)) = self.link_under_cursor() else {
            return;
        };
        let Some(anchor_intra) = self.line_map.position_for_index(row, link.start) else {
            return;
        };
        let (base_x, base_y) = self.row_base_xy(row);
        let anchor = point(
            text_origin.x + base_x + anchor_intra.x,
            text_origin.y + base_y + anchor_intra.y,
        );

        let label = "\u{2197} Visit".to_string();
        let mut font = window.text_style().font();
        font.family = SharedString::new(FONT_UI);
        let color = token_hsla(self.theme.link);
        let run = TextRun {
            len: label.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let mut shaped = window
            .text_system()
            .shape_text(SharedString::new(label.clone()), px(12.0), &[run], None, None)
            .unwrap_or_default();
        let Some(text_line) = shaped.pop() else {
            return;
        };

        let pad_x = px(8.0);
        let text_height = px(16.0);
        let button_height = px(22.0);
        let button_width = text_line.size(text_height).width + pad_x * 2.0;
        let gap = px(4.0);

        // Float above the link; if there isn't room (top rows), drop below it.
        let mut top = anchor.y - gap - button_height;
        if top < bounds.top() + px(2.0) {
            top = anchor.y + self.line_map.line_text_height(row) + gap;
        }
        let button_bounds = Bounds::new(point(anchor.x, top), size(button_width, button_height));

        window.paint_quad(quad(
            button_bounds,
            px(6.0),
            token_rgba(self.theme.bg_modal),
            px(1.0),
            token_hsla(self.theme.border_main),
            BorderStyle::default(),
        ));
        let _ = text_line.paint(
            point(anchor.x + pad_x, top + px(3.0)),
            text_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );

        self.open_link_button = Some(LinkHitbox {
            bounds: button_bounds,
            url,
        });
    }

    /// Records clickable regions for any URLs on `row`. A link that wraps across
    /// visual lines gets one hitbox per wrapped segment. The painted underline
    /// and color come from the layout's text runs; this only does hit-testing.
    pub(in crate::scheme_editor) fn register_link_hitboxes(&mut self, row: usize, line_origin: Point<Pixels>) {
        let Some(range) = self.line_range(row) else {
            return;
        };
        let Some(line) = self.text.get(range).map(str::to_string) else {
            return;
        };
        let links = detect_links(&line);
        if links.is_empty() {
            return;
        }
        let line_height = self.line_map.row_line_height(row);
        let wrap_ranges = self.line_map.wrapped_line_ranges(row);
        for link in links {
            let url = link_url(&line[link.clone()]);
            for wrap in &wrap_ranges {
                let start = link.start.max(wrap.start);
                let end = link.end.min(wrap.end);
                if start >= end {
                    continue;
                }
                let (Some(p0), Some(p1)) = (
                    self.line_map.position_for_index(row, start),
                    self.line_map.position_for_index(row, end),
                ) else {
                    continue;
                };
                let width = (p1.x - p0.x).max(px(0.0));
                if width <= px(0.0) {
                    continue;
                }
                self.link_hitboxes.push(LinkHitbox {
                    bounds: Bounds::new(
                        point(line_origin.x + p0.x, line_origin.y + p0.y),
                        size(width, line_height),
                    ),
                    url: url.clone(),
                });
            }
        }
    }
}
