use super::*;

impl SchemeEditor {
    /// Rows whose markdown markers should be revealed (the cursor/selection
    /// lines). `None` means no line is active, so every line renders collapsed
    /// (e.g. read-only schemes).
    pub(super) fn active_preview_rows(&self) -> Option<(usize, usize)> {
        if self.read_only || !self.editor_focused {
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
        let mut new_table_layouts = HashMap::new();
        self.last_active_rows = self.active_preview_rows();

        for (row, line) in self.text_lines().into_iter().enumerate() {
            let path = self.rows.get(row).map(|row| row.path).unwrap_or_default();
            let is_anchor = path.is_table_anchor();
            let is_cell = path.is_cell();
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

            let block_height = if is_anchor {
                row_item
                    .and_then(|item| {
                        self.build_table_layout(
                            item,
                            self.table_content_width(row, wrap_width),
                            window,
                        )
                    })
                    .map(|layout| {
                        let height = layout.block_height;
                        new_table_layouts.insert(row, layout);
                        height
                    })
                    .unwrap_or(px(0.0))
            } else {
                px(0.0)
            };

            let row_layout_offset = px(HANGING_WRAP_X_OFFSET);
            let text_width = if is_cell {
                self.table_layouts_lookup(&new_table_layouts, path.anchor, path.c)
                    - self.row_indent_x(row)
                    - if hidden_prefix.is_empty() {
                        px(0.0)
                    } else {
                        px(CHECKBOX_SIZE + CHECKBOX_GAP)
                    }
            } else {
                wrap_width - px(TEXT_LEFT_PAD + 18.0) - self.row_indent_x(row) - row_layout_offset
            }
            .max(px(40.0));
            let line_without_table = line_without_table_object(&line);
            let has_line_text = !line_without_table.is_empty();
            let media_height = if is_cell || is_anchor {
                px(0.0)
            } else {
                row_item
                    .map(|item| self.media_stack_height(item, text_width, has_line_text))
                    .unwrap_or(px(0.0))
            };
            let is_done = row_item.map(item_is_done).unwrap_or(false);
            let color = if is_done {
                token_hsla(self.theme.done_text)
            } else if path.is_header_cell() {
                self.table_header_text_color()
            } else {
                token_hsla(self.theme.text_primary)
            };
            let (font_size, mut line_height) =
                if !is_cell {
                    match markdown_heading_level(&line_without_table) {
                        Some(1) => (HEADING_FONT_SIZE, HEADING_LINE_HEIGHT),
                        Some(2) => (HEADING2_FONT_SIZE, HEADING2_LINE_HEIGHT),
                        Some(_) => (HEADING3_FONT_SIZE, HEADING3_LINE_HEIGHT),
                        None => (TEXT_FONT_SIZE, TEXT_LINE_HEIGHT),
                    }
                } else {
                    (TEXT_FONT_SIZE, TEXT_LINE_HEIGHT)
                };
            // Collapse the empty text band for a line whose only content is a
            // block inline (a table anchor, or an image-only line) so the image
            // or table renders in place instead of hanging below a full-height
            // blank text line.
            let media_only = !is_cell
                && !is_anchor
                && !has_line_text
                && row_item.map(|item| item.has_images()).unwrap_or(false);
            if (is_anchor && !has_line_text) || media_only {
                line_height = 2.0;
            }
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
                .with_block_height(block_height)
                .in_grid(is_cell)
                .with_layout_mapping(hidden_prefix.len(), collapsed, line.len()),
            );
        }

        self.line_map.replace_lines(wrapped);
        self.table_layouts = new_table_layouts;
        self.compute_cell_slots();
        self.line_map_dirty = false;
    }

    fn table_layouts_lookup(
        &self,
        layouts: &HashMap<usize, super::table::TableLayout>,
        anchor: usize,
        col: usize,
    ) -> Pixels {
        layouts
            .get(&anchor)
            .and_then(|layout| layout.col_w.get(col).copied())
            .map(|width| (width - px(super::table::CELL_PAD_X * 2.0)).max(px(16.0)))
            .unwrap_or(px(120.0))
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
            let mut segment_start = pos;
            for (relative, ch) in line[pos..end].char_indices() {
                if ch != TABLE_OBJECT_CHAR {
                    continue;
                }
                let object_start = pos + relative;
                push_line_layout_segment(
                    self,
                    line,
                    segment_start..object_start,
                    is_marker,
                    reveal,
                    markdown_run.style,
                    font,
                    default_color,
                    is_done,
                    &mut shaped,
                    &mut runs,
                    &mut collapsed,
                );
                collapsed.push(object_start..object_start + TABLE_OBJECT_LEN);
                segment_start = object_start + TABLE_OBJECT_LEN;
            }
            push_line_layout_segment(
                self,
                line,
                segment_start..end,
                is_marker,
                reveal,
                markdown_run.style,
                font,
                default_color,
                is_done,
                &mut shaped,
                &mut runs,
                &mut collapsed,
            );
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

fn push_line_layout_segment(
    editor: &SchemeEditor,
    line: &str,
    range: Range<usize>,
    is_marker: bool,
    reveal: bool,
    style: MarkdownStyle,
    font: &gpui::Font,
    default_color: gpui::Hsla,
    is_done: bool,
    shaped: &mut String,
    runs: &mut Vec<TextRun>,
    collapsed: &mut Vec<Range<usize>>,
) {
    if range.is_empty() {
        return;
    }
    if is_marker && !reveal {
        collapsed.push(range);
        return;
    }
    shaped.push_str(&line[range.clone()]);
    runs.push(editor.text_run(
        range.end - range.start,
        font,
        default_color,
        style,
        is_done,
    ));
}
