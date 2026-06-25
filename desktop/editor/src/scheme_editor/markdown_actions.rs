use super::*;

impl SchemeEditor {
    pub(super) fn toggle_bold(&mut self, cx: &mut Context<Self>) {
        self.toggle_wrapped_markdown("**", cx);
    }

    pub(super) fn toggle_italic(&mut self, cx: &mut Context<Self>) {
        self.toggle_wrapped_markdown("__", cx);
    }

    pub(super) fn toggle_highlight(&mut self, cx: &mut Context<Self>) {
        self.toggle_wrapped_markdown("==", cx);
    }

    pub(super) fn toggle_strikethrough(&mut self, cx: &mut Context<Self>) {
        self.toggle_wrapped_markdown("~~", cx);
    }

    pub(super) fn toggle_wrapped_markdown(&mut self, delimiter: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let range = if self.selection.is_empty() {
            self.line_range(self.current_row_index()).unwrap_or(0..0)
        } else {
            let (start, end) = self.selection_offsets();
            start..end
        };
        let Some(selected) = self.text.get(range.clone()) else {
            return;
        };
        let already_wrapped = selected.len() >= delimiter.len() * 2
            && selected.starts_with(delimiter)
            && selected.ends_with(delimiter)
            && selected.is_char_boundary(delimiter.len())
            && selected.is_char_boundary(selected.len() - delimiter.len());
        let replacement = if already_wrapped {
            selected[delimiter.len()..selected.len() - delimiter.len()].to_string()
        } else {
            format!("{delimiter}{selected}{delimiter}")
        };
        let start = range.start;
        self.replace_byte_range(range, &replacement, None, cx);
        // When adding the markers, place the cursor just inside the closing
        // delimiter (right before the trailing `**`/`__`/`==`/`~~`) rather than
        // after it, so continued typing stays within the emphasized run.
        if !already_wrapped {
            let cursor_offset = start + replacement.len() - delimiter.len();
            self.selection =
                TextSelection::collapsed(self.clamp_location(self.offset_to_location(cursor_offset)));
            self.scroll_to_cursor(cx);
            cx.notify();
        }
    }

    pub(super) fn toggle_heading(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let (start_row, end_row) = self.selected_row_range();
        let ranges = line_ranges(&self.text);
        if ranges.is_empty() {
            return;
        }

        let all_heading = (start_row..=end_row).all(|row| {
            ranges
                .get(row)
                .and_then(|range| self.text.get(range.clone()))
                .is_some_and(is_markdown_heading)
        });

        let mut text = self.text.clone();
        let cursor_after = self.selection.head;
        for row in (start_row..=end_row).rev() {
            let Some(range) = ranges.get(row).cloned() else {
                continue;
            };
            let Some(line) = text.get(range.clone()) else {
                continue;
            };
            if all_heading {
                if let Some(remove) = markdown_heading_marker_range(line) {
                    text.replace_range(range.start + remove.start..range.start + remove.end, "");
                }
            } else if !is_markdown_heading(line) {
                text.insert_str(range.start, "# ");
            }
        }
        self.sync_text_from_buffer(text, cursor_after, None, true, None, cx);
    }

    pub(super) fn active_text_is_bold(&self) -> bool {
        self.active_text_is_wrapped("**")
    }

    pub(super) fn active_text_is_italic(&self) -> bool {
        self.active_text_is_wrapped("__")
    }

    pub(super) fn active_text_is_highlight(&self) -> bool {
        self.active_text_is_wrapped("==")
    }

    pub(super) fn active_text_is_strikethrough(&self) -> bool {
        self.active_text_is_wrapped("~~")
    }

    pub(super) fn active_text_is_wrapped(&self, delimiter: &str) -> bool {
        let range = if self.selection.is_empty() {
            self.line_range(self.current_row_index()).unwrap_or(0..0)
        } else {
            let (start, end) = self.selection_offsets();
            start..end
        };
        self.text
            .get(range)
            .map(|text| {
                let text = text.trim();
                text.len() >= delimiter.len() * 2
                    && text.starts_with(delimiter)
                    && text.ends_with(delimiter)
            })
            .unwrap_or(false)
    }

    pub(super) fn active_text_is_heading(&self) -> bool {
        let (start_row, end_row) = self.selected_row_range();
        (start_row..=end_row).all(|row| {
            self.line_range(row)
                .and_then(|range| self.text.get(range))
                .is_some_and(is_markdown_heading)
        })
    }
}
