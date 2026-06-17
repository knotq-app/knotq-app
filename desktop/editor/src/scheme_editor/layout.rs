use super::*;

impl SchemeEditor {
    /// Rows whose markdown markers should be revealed (the cursor/selection
    /// lines). `None` means no line is active, so every line renders collapsed
    /// (e.g. read-only schemes).
    pub(super) fn active_preview_rows(&self) -> Option<(usize, usize)> {
        if self.read_only {
            return None;
        }
        let a = self.selection.anchor.row;
        let b = self.selection.head.row;
        Some((a.min(b), a.max(b)))
    }

    fn row_reveals_markers(&self, row: usize) -> bool {
        matches!(self.active_preview_rows(), Some((start, end)) if row >= start && row <= end)
    }

    pub(super) fn relayout(&mut self, wrap_width: Pixels, window: &mut Window) {
        let font = window.text_style().font();
        let mut wrapped = Vec::new();
        self.last_active_rows = self.active_preview_rows();

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
            let (font_size, line_height) = if is_markdown_heading(&line) {
                (HEADING_FONT_SIZE, HEADING_LINE_HEIGHT)
            } else {
                (TEXT_FONT_SIZE, TEXT_LINE_HEIGHT)
            };
            let reveal = self.row_reveals_markers(row);
            let (shaped_text, runs, collapsed) =
                self.build_line_layout(&font, hidden_prefix, &line, color, is_done, reveal);
            let mut shaped = window
                .text_system()
                .shape_text(
                    SharedString::new(shaped_text),
                    px(font_size),
                    &runs,
                    Some(text_width),
                    None,
                )
                .unwrap_or_default();
            wrapped.push(
                SchemeItemLine::new(
                    shaped.pop().unwrap_or_default(),
                    annotation,
                    px(line_height),
                )
                .with_media_height(media_height)
                .with_layout_mapping(hidden_prefix.len(), collapsed, line.len()),
            );
        }

        self.line_map.replace_lines(wrapped);
        self.line_map_dirty = false;
    }

    /// Builds the shaped string, its text runs, and the set of collapsed buffer
    /// ranges for one line. When `reveal` is false, markdown marker runs (`*`,
    /// `==`, leading `#`) are dropped from the layout and recorded as collapsed,
    /// so the markers take no visual space while the cursor is elsewhere.
    pub(super) fn build_line_layout(
        &self,
        font: &gpui::Font,
        hidden_prefix: &str,
        line: &str,
        default_color: gpui::Hsla,
        is_done: bool,
        reveal: bool,
    ) -> (String, Vec<TextRun>, Vec<Range<usize>>) {
        let mut shaped = String::with_capacity(hidden_prefix.len() + line.len());
        let mut runs = Vec::new();
        let mut collapsed = Vec::new();

        if !hidden_prefix.is_empty() {
            shaped.push_str(hidden_prefix);
            runs.push(self.text_run(
                hidden_prefix.len(),
                font,
                default_color,
                MarkdownStyle::default(),
                false,
            ));
        }

        let mut pos = 0;
        for markdown_run in parse_markdown_runs(line) {
            let end = pos + markdown_run.len;
            let is_marker = markdown_run.kind == MarkdownRunKind::Marker;
            if is_marker && !reveal {
                collapsed.push(pos..end);
            } else {
                shaped.push_str(&line[pos..end]);
                runs.push(self.text_run(
                    markdown_run.len,
                    font,
                    default_color,
                    markdown_run.style,
                    is_done,
                ));
            }
            pos = end;
        }

        if runs.is_empty() {
            runs.push(self.text_run(0, font, default_color, MarkdownStyle::default(), is_done));
        }

        (shaped, runs, collapsed)
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

        // Highlighted text keeps its normal color (Obsidian-style): the
        // translucent `highlight_bg` tints the line without recoloring glyphs,
        // which reads correctly on both light and dark themes.
        let color = if is_done {
            token_hsla(self.theme.done_text)
        } else {
            default_color
        };

        TextRun {
            len,
            font,
            color,
            background_color: if style.highlight {
                Some(token_hsla(self.theme.highlight_bg))
            } else {
                None
            },
            underline: None,
            // Completed items strike through the whole line; `~~text~~` strikes
            // just its span. Either way the rule follows the run's text color.
            strikethrough: if is_done || style.strikethrough {
                Some(gpui::StrikethroughStyle {
                    color: Some(color),
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
