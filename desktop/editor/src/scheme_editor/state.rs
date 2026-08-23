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
            editor.editor_focused = true;
            editor.reset_cursor_blink(cx);
            cx.emit(EditorEvent::Focused {
                scheme_id: editor.scheme_id,
            });
        });
        let focus_out_subscription =
            cx.on_focus_out(&focus_handle, window, |editor, _event, _window, cx| {
                editor.editor_focused = false;
                editor.is_selecting = false;
                editor.mouse_selection_mode = None;
                editor.mouse_selection_origin = None;
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
            synced_revision: None,
            rows,
            text,
            selection: initial_selection,
            marked_range: None,
            is_selecting: false,
            shape_cache: Default::default(),
            editor_focused: false,
            mouse_selection_mode: None,
            mouse_selection_origin: None,
            cursor_blink_state: true,
            cursor_blink_task: None,
            focus_handle,
            _focus_in_subscription: focus_in_subscription,
            _focus_out_subscription: focus_out_subscription,
            line_map: LineMap::new(px(TEXT_LINE_HEIGHT)),
            line_map_dirty: true,
            last_active_rows: None,
            pending_scroll_to_cursor: true,
            last_bounds: None,
            scroll_handle,
            top_pad: TEXT_TOP_PAD,
            bottom_pad: TEXT_BOTTOM_PAD,
            checkbox_hitboxes: Vec::new(),
            date_annotation_hitboxes: Vec::new(),
            repeat_annotation_hitboxes: Vec::new(),
            link_hitboxes: Vec::new(),
            open_link_button: None,
            hovered_link: false,
            auto_scroll_task: None,
            auto_scroll_last_mouse_position: None,
            image_cache: HashMap::new(),
            auto_bullet_undo: None,
            table_layouts: HashMap::new(),
            cell_slots: HashMap::new(),
            table_control_hitboxes: Vec::new(),
            hovered_table_control: None,
            remote_cursors: Vec::new(),
        }
    }

    /// The local caret as `(item id, char offset)` — item-relative so it can be
    /// broadcast as presence and rendered correctly on devices with a different
    /// row layout. `None` when the caret isn't on a known item row.
    pub fn caret_presence(&self) -> Option<(ItemId, usize)> {
        let head = self.selection.head;
        let item_id = self.rows.get(head.row).map(|row| row.item.id)?;
        Some((item_id, head.col))
    }

    /// Replace the set of remote peer carets shown in this editor. Cheap no-op when
    /// unchanged so it can be called every render.
    pub fn set_remote_cursors(&mut self, cursors: Vec<RemoteCursor>, cx: &mut Context<Self>) {
        if self.remote_cursors == cursors {
            return;
        }
        self.remote_cursors = cursors;
        cx.notify();
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.editor_focused = true;
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
        // Moving the cursor to a different line changes which line reveals its
        // markdown markers, so reshape when the active row range changes.
        if self.active_preview_rows() != self.last_active_rows {
            self.line_map_dirty = true;
        }
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
            strikethrough: self.active_text_is_strikethrough(),
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

    /// Override which glyphs the selected lines' markers draw from.
    ///
    /// Only applied to lines the family suits — a numbered family on a bullet
    /// line has nothing to draw — so a multi-line selection of mixed markers
    /// changes the ones it can and leaves the rest alone rather than doing
    /// nothing or doing something wrong.
    pub fn set_marker_family_for_selection(
        &mut self,
        family: knotq_model::MarkerFamily,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let (start_row, end_row) = self.selected_row_range();
        let mut commands = Vec::new();
        for row in start_row..=end_row {
            let Some(editor_row) = self.rows.get_mut(row) else {
                continue;
            };
            if !family.is_valid_for(editor_row.item.marker) {
                continue;
            }
            if editor_row.item.marker_family == family {
                continue;
            }
            editor_row.item.marker_family = family;
            commands.push(Command::SetItemMarkerFamily {
                scheme: self.scheme_id,
                item: editor_row.item.id,
                family,
            });
        }
        self.refresh_layout_after_content_change(None);
        self.emit_commands(commands, cx);
    }

    /// The family shown as active in the picker: the one every selected line
    /// that could carry a family agrees on, or the default when they differ.
    pub fn selection_marker_family(&self) -> knotq_model::MarkerFamily {
        let (start_row, end_row) = self.selected_row_range();
        let mut found: Option<knotq_model::MarkerFamily> = None;
        for row in start_row..=end_row {
            let Some(editor_row) = self.rows.get(row) else {
                continue;
            };
            if knotq_model::MarkerFamily::choices_for(editor_row.item.marker).is_empty() {
                continue;
            }
            match found {
                None => found = Some(editor_row.item.marker_family),
                Some(existing) if existing == editor_row.item.marker_family => {}
                Some(_) => return knotq_model::MarkerFamily::Standard,
            }
        }
        found.unwrap_or(knotq_model::MarkerFamily::Standard)
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

    pub fn toggle_strikethrough_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.toggle_strikethrough(cx);
    }

    pub fn toggle_heading_from_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.toggle_heading(cx);
    }

    pub fn insert_image_from_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.insert_image_from_picker(window, cx);
    }

    pub fn insert_table_from_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.insert_table(window, cx);
    }

    pub fn indent_from_toolbar(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.indent_current_line(delta, cx);
    }

    /// Reconcile the editor with the scheme it is showing.
    ///
    /// `revision` is the workspace content revision `scheme` came from. Rebuilding
    /// the buffer walks every item, and the view re-renders far more often than
    /// the content changes (cursor blink, hover, sibling panels), so an unchanged
    /// revision with unchanged presentation skips the rebuild entirely. Pass
    /// `None` to force the rebuild.
    pub fn sync_from_scheme(
        &mut self,
        scheme: &Scheme,
        revision: Option<u64>,
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
        let scheme_changed = self.scheme_id != scheme.id;
        self.scheme_id = scheme.id;

        // Nothing that feeds the buffer moved since the last rebuild.
        let unchanged = !scheme_changed
            && !theme_changed
            && !time_format_changed
            && !color_changed
            && revision.is_some()
            && self.synced_revision == revision;
        if unchanged {
            self.relayout_if_dirty(window);
            return;
        }
        self.synced_revision = revision;

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
