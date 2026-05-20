use super::*;

impl SchemeEditor {
    pub(super) fn relayout(&mut self, wrap_width: Pixels, window: &mut Window) {
        let font = window.text_style().font();
        let mut wrapped = Vec::new();

        for (row, line) in self.text_lines().into_iter().enumerate() {
            let row_item = self.rows.get(row).map(|row| &row.item);
            let annotation = row_item.and_then(|item| {
                annotation_text(item, self.time_format).map(|text| SchemeItemAnnotation {
                    text,
                    height: px(ANNOTATION_HEIGHT),
                })
            });
            let hidden_prefix = if row_item.is_some_and(|item| item.marker == ItemMarker::Blank) {
                ""
            } else {
                HANGING_WRAP_PREFIX
            };
            let row_layout_offset = px(HANGING_WRAP_X_OFFSET);
            let text_width = (wrap_width
                - px(TEXT_LEFT_PAD + 18.0)
                - self.row_indent_x(row)
                - row_layout_offset)
                .max(px(120.0));
            let media_height = row_item
                .map(|item| self.media_stack_height(item, text_width))
                .unwrap_or(px(0.0));
            let is_done = row_item.map(item_is_done).unwrap_or(false);
            let color = if is_done {
                token_hsla(self.theme.done_text)
            } else {
                token_hsla(self.theme.text_primary)
            };
            let shaped_text = if hidden_prefix.is_empty() {
                line.clone()
            } else {
                format!("{hidden_prefix}{line}")
            };
            let runs = self.markdown_text_runs(&font, hidden_prefix.len(), &line, color, is_done);
            let mut shaped = window
                .text_system()
                .shape_text(
                    SharedString::new(shaped_text),
                    px(TEXT_FONT_SIZE),
                    &runs,
                    Some(text_width),
                    None,
                )
                .unwrap_or_default();
            wrapped.push(
                SchemeItemLine::new(shaped.pop().unwrap_or_default(), annotation)
                    .with_media_height(media_height)
                    .with_hidden_prefix(hidden_prefix.len()),
            );
        }

        self.line_map.replace_lines(wrapped);
        self.line_map_dirty = false;
    }

    pub(super) fn markdown_text_runs(
        &self,
        font: &gpui::Font,
        hidden_prefix_len: usize,
        line: &str,
        default_color: gpui::Hsla,
        is_done: bool,
    ) -> Vec<TextRun> {
        let mut runs = Vec::new();
        if hidden_prefix_len > 0 {
            runs.push(self.text_run(
                hidden_prefix_len,
                font,
                default_color,
                MarkdownStyle::default(),
                false,
            ));
        }

        for markdown_run in parse_markdown_runs(line) {
            runs.push(self.text_run(
                markdown_run.len,
                font,
                default_color,
                markdown_run.style,
                is_done,
            ));
        }

        if runs.is_empty() {
            runs.push(self.text_run(0, font, default_color, MarkdownStyle::default(), is_done));
        }

        runs
    }

    pub(super) fn text_run(
        &self,
        len: usize,
        font: &gpui::Font,
        default_color: gpui::Hsla,
        style: MarkdownStyle,
        is_done: bool,
    ) -> TextRun {
        let mut font = font.clone();
        font.family = SharedString::new(FONT_UI);
        if style.bold {
            font.weight = gpui::FontWeight::BOLD;
        }
        if style.italic {
            font.style = gpui::FontStyle::Italic;
        }

        let color = if is_done {
            token_hsla(self.theme.done_text)
        } else {
            default_color
        };

        TextRun {
            len,
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: if is_done {
                Some(gpui::StrikethroughStyle {
                    color: Some(token_hsla(self.theme.done_text)),
                    thickness: px(1.0),
                })
            } else {
                None
            },
        }
    }

    pub fn estimated_height(&self) -> Pixels {
        let rough = px(TEXT_LINE_HEIGHT * self.render_line_count() as f32);
        if self.line_map_dirty {
            return px(self.top_pad + self.bottom_pad) + rough.max(px(TEXT_LINE_HEIGHT));
        }
        let shaped = self.line_map.total_height();
        px(self.top_pad + self.bottom_pad) + shaped.max(rough).max(px(TEXT_LINE_HEIGHT))
    }
}
