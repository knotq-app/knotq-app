use super::*;

/// Read-only inputs for [`SchemeEditor::build_line_layout`]. Output
/// accumulators (`shaped`, `runs`, `collapsed`) stay as separate `&mut`
/// params on the callee so the borrow checker doesn't need to reason about
/// aliasing between this struct and the accumulators.
pub(super) struct LineLayoutInput<'a> {
    pub font: &'a gpui::Font,
    pub hidden_prefix: &'a str,
    pub line: &'a str,
    pub default_color: gpui::Hsla,
    pub is_done: bool,
    pub reveal: bool,
    pub extra_collapsed: &'a [Range<usize>],
}

/// Read-only inputs for [`push_line_layout_segment`]. See [`LineLayoutInput`]
/// for why the output accumulators are kept separate.
struct LineSegmentInput<'a> {
    editor: &'a SchemeEditor,
    line: &'a str,
    range: Range<usize>,
    is_marker: bool,
    reveal: bool,
    style: MarkdownStyle,
    font: &'a gpui::Font,
    default_color: gpui::Hsla,
    is_done: bool,
    extra_collapsed: &'a [Range<usize>],
    link_ranges: &'a [Range<usize>],
}

/// Read-only inputs for [`push_visible_line_layout_segment`]. Same shape as
/// [`LineSegmentInput`] minus `extra_collapsed`, which is already consumed by
/// the caller before recursing into the "visible" segment helper.
struct VisibleLineSegmentInput<'a> {
    editor: &'a SchemeEditor,
    line: &'a str,
    range: Range<usize>,
    is_marker: bool,
    reveal: bool,
    style: MarkdownStyle,
    font: &'a gpui::Font,
    default_color: gpui::Hsla,
    is_done: bool,
    link_ranges: &'a [Range<usize>],
}

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
        // Column headers render in a semibold weight so they stand out from the
        // body cells beyond just the (muted) header text color.
        let header_font = {
            let mut header_font = font.clone();
            header_font.weight = gpui::FontWeight::SEMIBOLD;
            header_font
        };
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
            let line_without_block = line_without_table_object(&line);
            let has_line_text = !line_without_block.is_empty();
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
            let (font_size, mut line_height) = if !is_cell {
                match markdown_heading_level(&line_without_block) {
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
            let suffix_range = if is_cell {
                None
            } else {
                block_suffix_range(&line)
            };
            let table_suffix = suffix_range.as_ref().and_then(|range| {
                let suffix = line[range.clone()].to_string();
                (!suffix.is_empty()).then_some(suffix)
            });
            let extra_collapsed = suffix_range.iter().cloned().collect::<Vec<_>>();
            let reveal = self.row_reveals_markers(row);
            let line_font = if path.is_header_cell() {
                &header_font
            } else {
                &font
            };
            let (shaped_text, runs, collapsed) = self.build_line_layout(LineLayoutInput {
                font: line_font,
                hidden_prefix,
                line: &line,
                default_color: color,
                is_done,
                reveal,
                extra_collapsed: &extra_collapsed,
            });
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
            let block_suffix = table_suffix.map(|suffix| {
                let suffix_width = new_table_layouts
                    .get(&row)
                    .map(|layout| layout.grid_w)
                    .unwrap_or(text_width)
                    .max(px(40.0));
                let run = self.text_run(
                    suffix.len(),
                    &font,
                    token_hsla(self.theme.text_primary),
                    MarkdownStyle::default(),
                    is_done,
                );
                window
                    .text_system()
                    .shape_text(
                        SharedString::new(suffix),
                        px(TEXT_FONT_SIZE),
                        &[run],
                        Some(suffix_width),
                        None,
                    )
                    .unwrap_or_default()
                    .pop()
                    .unwrap_or_default()
            });
            wrapped.push(
                SchemeItemLine::new(
                    shaped.pop().unwrap_or_default(),
                    annotation,
                    px(line_height),
                )
                .with_media_height(media_height)
                .with_block_height(block_height)
                .with_block_suffix(block_suffix, px(6.0))
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
        input: LineLayoutInput<'_>,
    ) -> (String, Vec<TextRun>, Vec<Range<usize>>) {
        let LineLayoutInput {
            font,
            hidden_prefix,
            line,
            default_color,
            is_done,
            reveal,
            extra_collapsed,
        } = input;
        let mut shaped = String::with_capacity(hidden_prefix.len() + line.len());
        let mut runs = Vec::new();
        let mut collapsed = Vec::new();
        let link_ranges = detect_links(line);

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
                    LineSegmentInput {
                        editor: self,
                        line,
                        range: segment_start..object_start,
                        is_marker,
                        reveal,
                        style: markdown_run.style,
                        font,
                        default_color,
                        is_done,
                        extra_collapsed,
                        link_ranges: &link_ranges,
                    },
                    &mut shaped,
                    &mut runs,
                    &mut collapsed,
                );
                collapsed.push(object_start..object_start + TABLE_OBJECT_LEN);
                segment_start = object_start + TABLE_OBJECT_LEN;
            }
            push_line_layout_segment(
                LineSegmentInput {
                    editor: self,
                    line,
                    range: segment_start..end,
                    is_marker,
                    reveal,
                    style: markdown_run.style,
                    font,
                    default_color,
                    is_done,
                    extra_collapsed,
                    link_ranges: &link_ranges,
                },
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
        // which reads correctly on both light and dark themes. Links override
        // the color so a URL reads as clickable on any line.
        let color = if is_done {
            token_hsla(self.theme.done_text)
        } else if style.link {
            token_hsla(self.theme.link)
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
            underline: style.link.then(|| gpui::UnderlineStyle {
                color: Some(color),
                thickness: px(1.0),
                wavy: false,
            }),
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
    input: LineSegmentInput<'_>,
    shaped: &mut String,
    runs: &mut Vec<TextRun>,
    collapsed: &mut Vec<Range<usize>>,
) {
    let LineSegmentInput {
        editor,
        line,
        range,
        is_marker,
        reveal,
        style,
        font,
        default_color,
        is_done,
        extra_collapsed,
        link_ranges,
    } = input;
    if range.is_empty() {
        return;
    }
    let mut start = range.start;
    for hidden in extra_collapsed {
        let hidden_start = hidden.start.max(range.start);
        let hidden_end = hidden.end.min(range.end);
        if hidden_start >= hidden_end {
            continue;
        }
        push_visible_line_layout_segment(
            VisibleLineSegmentInput {
                editor,
                line,
                range: start..hidden_start,
                is_marker,
                reveal,
                style,
                font,
                default_color,
                is_done,
                link_ranges,
            },
            shaped,
            runs,
            collapsed,
        );
        collapsed.push(hidden_start..hidden_end);
        start = hidden_end;
    }
    push_visible_line_layout_segment(
        VisibleLineSegmentInput {
            editor,
            line,
            range: start..range.end,
            is_marker,
            reveal,
            style,
            font,
            default_color,
            is_done,
            link_ranges,
        },
        shaped,
        runs,
        collapsed,
    );
}

fn push_visible_line_layout_segment(
    input: VisibleLineSegmentInput<'_>,
    shaped: &mut String,
    runs: &mut Vec<TextRun>,
    collapsed: &mut Vec<Range<usize>>,
) {
    let VisibleLineSegmentInput {
        editor,
        line,
        range,
        is_marker,
        reveal,
        style,
        font,
        default_color,
        is_done,
        link_ranges,
    } = input;
    if range.is_empty() {
        return;
    }
    if is_marker && !reveal {
        collapsed.push(range);
        return;
    }
    shaped.push_str(&line[range.clone()]);
    // Split the visible run at link boundaries so each emitted `TextRun` is
    // wholly inside or wholly outside a URL, and carries the link styling.
    for (sub, is_link) in split_by_links(range, link_ranges) {
        let mut run_style = style;
        run_style.link = is_link;
        runs.push(editor.text_run(sub.end - sub.start, font, default_color, run_style, is_done));
    }
}

/// Partitions `range` into maximal sub-ranges each entirely inside or entirely
/// outside the (sorted, disjoint) `link_ranges`, preserving order.
fn split_by_links(range: Range<usize>, link_ranges: &[Range<usize>]) -> Vec<(Range<usize>, bool)> {
    let mut parts = Vec::new();
    let mut pos = range.start;
    for link in link_ranges {
        if link.end <= pos || link.start >= range.end {
            continue;
        }
        let link_start = link.start.max(range.start);
        let link_end = link.end.min(range.end);
        if link_start > pos {
            parts.push((pos..link_start, false));
        }
        parts.push((link_start..link_end, true));
        pos = link_end;
    }
    if pos < range.end {
        parts.push((pos..range.end, false));
    }
    parts
}
