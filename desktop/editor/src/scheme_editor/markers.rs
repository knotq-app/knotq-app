use super::*;

impl SchemeEditor {
    pub(super) fn paint_line_marker(
        &mut self,
        row: &EditorRow,
        row_ix: usize,
        line_origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = self.theme;
        let annotation_color = self.annotation_color();
        let annotation_rgba: gpui::Rgba = annotation_color.into();
        let text_left = line_origin.x + self.first_text_x(row_ix);
        let left = self.marker_left_for_text_left(&row.item, text_left);
        let line_height = self.line_map.row_line_height(row_ix);
        let top = line_origin.y + (line_height - px(CHECKBOX_SIZE)) / 2.0;
        let checkbox_bounds =
            Bounds::new(point(left, top), size(px(CHECKBOX_SIZE), px(CHECKBOX_SIZE)));
        let done = item_is_done(&row.item);
        let partial = item_is_partial(&row.item);

        self.paint_indentation_guides(&row.item, row_ix, line_origin, checkbox_bounds, window);

        match row.item.marker {
            ItemMarker::Blank => {}
            ItemMarker::Bullet => {
                let bullet = px(4.0);
                let bullet_bounds = Bounds::new(
                    point(
                        checkbox_bounds.left() + (checkbox_bounds.size.width - bullet) / 2.0,
                        checkbox_bounds.top() + (checkbox_bounds.size.height - bullet) / 2.0,
                    ),
                    size(bullet, bullet),
                );
                window.paint_quad(fill(bullet_bounds, annotation_color));
            }
            ItemMarker::Numbered => {
                if let Some(number) = numbered_marker_ordinal(&self.rows, row_ix) {
                    self.paint_numbered_marker(
                        &format!("{number}."),
                        text_left,
                        line_origin.y,
                        line_height,
                        annotation_color,
                        window,
                        cx,
                    );
                }
            }
            ItemMarker::Checkbox => {
                window.paint_quad(quad(
                    checkbox_bounds,
                    px(2.0),
                    if done {
                        annotation_rgba
                    } else {
                        token_rgba(theme.checkbox_fill_off)
                    },
                    px(1.0),
                    if done || partial {
                        annotation_color
                    } else {
                        token_hsla(theme.checkbox_border_off)
                    },
                    BorderStyle::default(),
                ));

                if done {
                    self.paint_checkbox_checkmark(checkbox_bounds, window);
                } else if partial {
                    self.paint_checkbox_partial_mark(checkbox_bounds, window);
                }

                self.checkbox_hitboxes.push(CheckboxHitbox {
                    bounds: checkbox_bounds,
                    item_id: row.item.id,
                });
            }
        }

        if annotation_text(&row.item, self.time_format).is_some() {
            let bar_x = self.annotation_bar_x(checkbox_bounds);
            let row_height = self
                .line_map
                .item_line(row_ix)
                .map(SchemeItemLine::height)
                .unwrap_or(line_height);
            let guide_margin = px(3.0);
            let previous_has_annotation = row_ix
                .checked_sub(1)
                .and_then(|ix| self.rows.get(ix))
                .is_some_and(|row| annotation_text(&row.item, self.time_format).is_some());
            let next_has_annotation = self
                .rows
                .get(row_ix + 1)
                .is_some_and(|row| annotation_text(&row.item, self.time_format).is_some());
            let top_margin = if previous_has_annotation {
                px(0.0)
            } else {
                guide_margin
            };
            let bottom_margin = if next_has_annotation {
                px(0.0)
            } else {
                guide_margin
            };
            window.paint_quad(fill(
                Bounds::new(
                    point(bar_x, line_origin.y + top_margin),
                    size(
                        px(1.0),
                        (row_height - top_margin - bottom_margin).max(px(1.0)),
                    ),
                ),
                annotation_color,
            ));
        }
    }

    pub(super) fn paint_indentation_guides(
        &self,
        item: &Item,
        row_ix: usize,
        line_origin: Point<Pixels>,
        checkbox_bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        let indent = item.indent.min(MAX_INDENT);
        if indent == 0 {
            return;
        }

        let guide_color = token_rgba(self.theme.divider_soft);
        let guide_margin = px(3.0);
        let row_height = self
            .line_map
            .item_line(row_ix)
            .map(SchemeItemLine::height)
            .unwrap_or_else(|| self.line_map.row_line_height(row_ix));
        let own_bar_x = self.annotation_bar_x(checkbox_bounds);

        for guide_indent in 1..=indent {
            let level_offset = px((indent - guide_indent) as f32 * INDENT_WIDTH);
            let previous_has_guide = row_ix
                .checked_sub(1)
                .and_then(|ix| self.rows.get(ix))
                .is_some_and(|row| row.item.indent.min(MAX_INDENT) >= guide_indent);
            let next_has_guide = self
                .rows
                .get(row_ix + 1)
                .is_some_and(|row| row.item.indent.min(MAX_INDENT) >= guide_indent);
            let top_margin = if previous_has_guide {
                px(0.0)
            } else {
                guide_margin
            };
            let bottom_margin = if next_has_guide {
                px(0.0)
            } else {
                guide_margin
            };

            window.paint_quad(fill(
                Bounds::new(
                    point(own_bar_x - level_offset, line_origin.y + top_margin),
                    size(
                        px(1.0),
                        (row_height - top_margin - bottom_margin).max(px(1.0)),
                    ),
                ),
                guide_color,
            ));
        }
    }

    pub(super) fn paint_numbered_marker(
        &self,
        label: &str,
        text_left: Pixels,
        y: Pixels,
        line_height: Pixels,
        color: gpui::Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut font = window.text_style().font();
        font.family = SharedString::new(FONT_UI);
        font.weight = gpui::FontWeight::MEDIUM;
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
            .shape_text(
                SharedString::new(label.to_string()),
                px(12.0),
                &[run],
                None,
                None,
            )
            .unwrap_or_default();
        let Some(line) = shaped.pop() else {
            return;
        };
        let width = line.size(line_height).width;
        let origin = point(text_left - px(CHECKBOX_GAP) - width, y);
        let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
    }

    pub(super) fn paint_checkbox_checkmark(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let mut path = PathBuilder::stroke(px(1.8));
        path.move_to(point(bounds.left() + px(3.2), bounds.top() + px(7.3)));
        path.line_to(point(bounds.left() + px(5.9), bounds.top() + px(9.8)));
        path.line_to(point(bounds.left() + px(10.8), bounds.top() + px(4.3)));
        if let Ok(path) = path.build() {
            window.paint_path(path, token_hsla(self.theme.checkbox_mark));
        }
    }

    pub(super) fn paint_checkbox_partial_mark(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let mut path = PathBuilder::stroke(px(1.7));
        path.move_to(point(bounds.left() + px(3.7), bounds.top() + px(7.0)));
        path.line_to(point(bounds.right() - px(3.7), bounds.top() + px(7.0)));
        if let Ok(path) = path.build() {
            window.paint_path(path, token_hsla(self.theme.checkbox_mark));
        }
    }

    pub(super) fn paint_date_annotation(
        &mut self,
        row: &EditorRow,
        row_ix: usize,
        line_origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(annotation_text) = self
            .line_map
            .item_line(row_ix)
            .and_then(|line| line.annotation.as_ref())
            .map(|annotation| annotation.text.as_str())
        else {
            return;
        };
        if annotation_text.is_empty() {
            return;
        }

        let parts = annotation_parts(&row.item, self.time_format).unwrap_or_default();
        let y = line_origin.y + self.line_map.line_text_height(row_ix) - px(2.0);
        let text_left = line_origin.x + self.first_text_x(row_ix);
        let checkbox_left = self.marker_left_for_text_left(&row.item, text_left);
        let mut x = checkbox_left - px(ANNOTATION_BAR_GAP + INDENT_GUIDE_X_SHIFT)
            + px(ANNOTATION_TEXT_GAP);
        let annotation_color = self.annotation_color();
        let mut painted = false;

        for (ix, (kind, label)) in parts.into_iter().enumerate() {
            if ix > 0 {
                x += self.paint_annotation_text(
                    " \u{2192} ",
                    point(x, y),
                    annotation_color,
                    window,
                    cx,
                );
            }

            let width =
                self.paint_annotation_text(&label, point(x, y), annotation_color, window, cx);
            self.date_annotation_hitboxes.push(DateAnnotationHitbox {
                bounds: Bounds::new(
                    point(x - px(3.0), y - px(2.0)),
                    size(width + px(6.0), px(ANNOTATION_HEIGHT)),
                ),
                item_id: row.item.id,
                kind,
            });
            x += width;
            painted = true;
        }

        if let Some(repeat) = row.item.repeats.as_ref() {
            if painted {
                x += self.paint_annotation_text(" · ", point(x, y), annotation_color, window, cx);
            }
            let label = format_repeat_annotation(repeat);
            let width =
                self.paint_annotation_text(&label, point(x, y), annotation_color, window, cx);
            self.repeat_annotation_hitboxes
                .push(RepeatAnnotationHitbox {
                    bounds: Bounds::new(
                        point(x - px(3.0), y - px(2.0)),
                        size(width + px(6.0), px(ANNOTATION_HEIGHT)),
                    ),
                    item_id: row.item.id,
                });
        }
    }

    pub(super) fn paint_annotation_text(
        &self,
        text: &str,
        origin: Point<Pixels>,
        color: gpui::Hsla,
        window: &mut Window,
        cx: &mut App,
    ) -> Pixels {
        let mut annotation_font = window.text_style().font();
        annotation_font.family = SharedString::new(FONT_MONO);
        let run = TextRun {
            len: text.len(),
            font: annotation_font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let mut shaped = window
            .text_system()
            .shape_text(
                SharedString::new(text.to_string()),
                px(ANNOTATION_FONT_SIZE),
                &[run],
                None,
                None,
            )
            .unwrap_or_default();
        let Some(line) = shaped.pop() else {
            return px(0.0);
        };
        let width = line.size(px(ANNOTATION_HEIGHT)).width;
        let _ = line.paint(
            origin,
            px(ANNOTATION_HEIGHT),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        width
    }

    pub(super) fn annotation_color(&self) -> gpui::Hsla {
        token_hsla(if self.theme.is_dark {
            0xb8c9e8ff
        } else {
            0x536a8fff
        })
    }

    fn annotation_bar_x(&self, checkbox_bounds: Bounds<Pixels>) -> Pixels {
        checkbox_bounds.left() - px(ANNOTATION_BAR_GAP + INDENT_GUIDE_X_SHIFT)
    }
}
