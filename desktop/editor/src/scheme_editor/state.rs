use super::*;

impl SchemeEditor {
    pub fn new(
        scheme_id: SchemeId,
        scheme: Scheme,
        theme: Theme,
        time_format: TimeFormat,
        scroll_handle: ScrollHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (text, rows) = build_buffer(&scheme.items);
        let initial_selection = TextSelection::collapsed(text_end_location(&text));
        let focus_handle = cx.focus_handle();
        let focus_in_subscription = cx.on_focus_in(&focus_handle, window, |editor, _window, cx| {
            editor.reset_cursor_blink(cx);
            cx.emit(EditorEvent::Focused {
                scheme_id: editor.scheme_id,
            });
        });
        let focus_out_subscription =
            cx.on_focus_out(&focus_handle, window, |editor, _event, _window, cx| {
                editor.is_selecting = false;
                editor.mouse_selection_mode = None;
                editor.stop_responding_to_mouse_movements();
                editor.cursor_blink_task = None;
                editor.cursor_blink_state = false;
                cx.notify();
            });
        Self {
            scheme_id,
            color_index: scheme.color_index,
            read_only: scheme.is_read_only(),
            theme,
            time_format,
            rows,
            text,
            selection: initial_selection,
            marked_range: None,
            is_selecting: false,
            mouse_selection_mode: None,
            cursor_blink_state: true,
            cursor_blink_task: None,
            focus_handle,
            _focus_in_subscription: focus_in_subscription,
            _focus_out_subscription: focus_out_subscription,
            line_map: LineMap::new(px(TEXT_LINE_HEIGHT)),
            line_map_dirty: true,
            pending_scroll_to_cursor: true,
            last_bounds: None,
            scroll_handle,
            top_pad: TEXT_TOP_PAD,
            bottom_pad: TEXT_BOTTOM_PAD,
            checkbox_hitboxes: Vec::new(),
            date_annotation_hitboxes: Vec::new(),
            repeat_annotation_hitboxes: Vec::new(),
            auto_scroll_task: None,
            auto_scroll_last_mouse_position: None,
            image_cache: HashMap::new(),
            auto_bullet_undo: None,
        }
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.focus_handle.focus(window);
        self.reset_cursor_blink(cx);
        cx.emit(EditorEvent::Focused {
            scheme_id: self.scheme_id,
        });
        cx.notify();
    }

    pub fn set_bottom_padding(&mut self, bottom_pad: f32, cx: &mut Context<Self>) {
        self.bottom_pad = bottom_pad;
        cx.notify();
    }

    pub fn set_top_padding(&mut self, top_pad: f32, cx: &mut Context<Self>) {
        self.top_pad = top_pad;
        self.line_map_dirty = true;
        cx.notify();
    }

    pub(super) fn refresh_layout_after_content_change(&mut self, window: Option<&mut Window>) {
        self.line_map_dirty = true;
        if let Some(window) = window {
            self.relayout_if_dirty(window);
        }
    }

    pub(super) fn relayout_if_dirty(&mut self, window: &mut Window) {
        if !self.line_map_dirty {
            return;
        }
        let wrap_width = self
            .last_bounds
            .map(|bounds| bounds.size.width)
            .filter(|width| *width > px(0.0))
            .or_else(|| {
                let width = self.scroll_handle.bounds().size.width;
                (width > px(0.0)).then_some(width)
            })
            .unwrap_or_else(|| window.viewport_size().width);
        self.relayout(wrap_width, window);
    }

    pub fn relayout_if_dirty_for_width(&mut self, wrap_width: Pixels, window: &mut Window) {
        if self.line_map_dirty {
            self.relayout(wrap_width, window);
        }
    }

    pub fn focus_item(&mut self, item_id: ItemId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.iter().position(|row| row.item.id == item_id) {
            self.selection = TextSelection::collapsed(TextLocation {
                row,
                col: self.line_len(row),
            });
            self.scroll_to_cursor(cx);
        }
        if self.read_only {
            cx.notify();
            return;
        }
        self.focus(window, cx);
    }

    pub fn session_state(&self) -> SchemeEditorSessionState {
        SchemeEditorSessionState {
            anchor: self.selection.anchor,
            head: self.selection.head,
        }
    }

    pub fn restore_session_state(
        &mut self,
        state: SchemeEditorSessionState,
        cx: &mut Context<Self>,
    ) {
        self.selection = TextSelection {
            anchor: self.clamp_location(state.anchor),
            head: self.clamp_location(state.head),
        };
        self.marked_range = None;
        self.cursor_blink_state = true;
        cx.emit(EditorEvent::SelectionChanged {
            scheme_id: self.scheme_id,
        });
        self.scroll_to_cursor(cx);
        cx.notify();
    }

    pub fn toolbar_state(&self) -> EditorToolbarState {
        let row = self.current_row_index();
        let item = self.rows.get(row).map(|row| &row.item);
        EditorToolbarState {
            marker: item.map(|item| item.marker).unwrap_or_default(),
            has_start: item.is_some_and(|item| item.start.is_some()),
            has_end: item.is_some_and(|item| item.end.is_some()),
            has_repeat: item.is_some_and(|item| item.repeats.is_some()),
            bold: self.active_text_is_bold(),
            italic: self.active_text_is_italic(),
            highlight: self.active_text_is_highlight(),
            heading: self.active_text_is_heading(),
        }
    }

    pub fn set_marker_for_selection(&mut self, marker: ItemMarker, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let (start_row, end_row) = self.selected_row_range();
        let mut commands = Vec::new();
        let mut cleared_checkbox_annotations = false;
        for row in start_row..=end_row {
            let Some(editor_row) = self.rows.get_mut(row) else {
                continue;
            };
            if editor_row.item.marker == marker {
                continue;
            }
            if marker == ItemMarker::Checkbox {
                editor_row.item.marker = marker;
                commands.push(Command::SetItemMarker {
                    scheme: self.scheme_id,
                    item: editor_row.item.id,
                    marker,
                });
            } else {
                let updated = item_with_marker(editor_row.item.clone(), marker);
                cleared_checkbox_annotations |= editor_row.item.start.is_some()
                    || editor_row.item.end.is_some()
                    || editor_row.item.available.is_some()
                    || editor_row.item.repeats.is_some();
                editor_row.item = updated.clone();
                commands.push(Command::ReplaceItem {
                    scheme: self.scheme_id,
                    item: updated,
                });
            }
        }
        if cleared_checkbox_annotations {
            cx.emit(EditorEvent::CloseDatePopover);
        }
        self.emit_commands(commands, cx);
    }

    pub fn toggle_start_date_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.open_date_for_current_line(DateKind::Start, cx);
    }

    pub fn toggle_end_date_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.open_date_for_current_line(DateKind::End, cx);
    }

    pub fn toggle_repeat_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.toggle_repeat_for_current_line(cx);
    }

    pub fn toggle_bold_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.toggle_bold(cx);
    }

    pub fn toggle_italic_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.toggle_italic(cx);
    }

    pub fn toggle_highlight_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.toggle_highlight(cx);
    }

    pub fn toggle_heading_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.toggle_heading(cx);
    }

    pub fn indent_from_toolbar(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.indent_current_line(delta, cx);
    }

    pub fn sync_from_scheme(
        &mut self,
        scheme: Scheme,
        theme: Theme,
        time_format: TimeFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme_changed = self.theme != theme;
        self.theme = theme;
        let color_changed = self.color_index != scheme.color_index;
        self.color_index = scheme.color_index;
        self.read_only = scheme.is_read_only();
        let time_format_changed = self.time_format != time_format;
        self.time_format = time_format;
        self.scheme_id = scheme.id;
        let (text, rows) = build_buffer(&scheme.items);
        if text != self.text
            || !same_rows(&rows, &self.rows)
            || time_format_changed
            || color_changed
            || theme_changed
        {
            self.text = text;
            self.rows = rows;
            self.refresh_layout_after_content_change(Some(window));
            self.selection = TextSelection::collapsed(self.clamp_location(self.selection.head));
            self.marked_range = None;
            self.scroll_to_cursor(cx);
            cx.notify();
        } else {
            self.relayout_if_dirty(window);
        }
    }
}
