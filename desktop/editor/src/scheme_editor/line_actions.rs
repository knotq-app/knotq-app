use super::*;

impl SchemeEditor {
    pub(super) fn indent_current_line(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let (start_row, end_row) = self.selected_row_range();
        let mut commands = Vec::new();

        for row in start_row..=end_row {
            let Some(editor_row) = self.rows.get_mut(row) else {
                continue;
            };
            let old_indent = editor_row.item.indent;
            let new_indent = if delta > 0 {
                old_indent.saturating_add(delta as u8).min(MAX_INDENT)
            } else {
                old_indent.saturating_sub(delta.unsigned_abs() as u8)
            };
            if old_indent == new_indent {
                continue;
            }
            editor_row.item.indent = new_indent;
            commands.push(Command::SetItemIndent {
                scheme: self.scheme_id,
                item: editor_row.item.id,
                indent: new_indent,
            });
        }

        self.emit_commands(commands, cx);
    }

    pub(super) fn open_date_for_current_line(&mut self, kind: DateKind, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let row = self.current_row_index();
        let Some(item) = self.rows.get(row).map(|row| row.item.clone()) else {
            return;
        };
        let anchor = self.date_anchor_for_row(row);
        cx.emit(EditorEvent::OpenDatePicker {
            scheme_id: self.scheme_id,
            item_id: item.id,
            kind,
            anchor,
        });
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn toggle_repeat_for_current_line(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let row = self.current_row_index();
        let Some(item) = self.rows.get(row).map(|row| row.item.clone()) else {
            return;
        };
        let anchor = self.date_anchor_for_row(row);
        cx.emit(EditorEvent::OpenRepeatPopover {
            scheme_id: self.scheme_id,
            item_id: item.id,
            anchor,
        });
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn toggle_status_for_current_line(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let row = self.current_row_index();
        let Some(item) = self.rows.get(row).map(|row| row.item.clone()) else {
            return;
        };
        cx.emit(EditorEvent::Command(Command::ToggleOccurrence {
            scheme: self.scheme_id,
            item: item.id,
            occurrence: OccurrenceId::Single,
        }));
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn remove_date_for_current_line(&mut self, kind: DateKind, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let row = self.current_row_index();
        let Some(item) = self.rows.get(row).map(|row| row.item.clone()) else {
            return;
        };
        cx.emit(EditorEvent::Command(Command::SetItemDate {
            scheme: self.scheme_id,
            item: item.id,
            kind,
            date: None,
        }));
        cx.emit(EditorEvent::CloseDatePopover);
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn date_anchor_for_row(&self, row: usize) -> Point<Pixels> {
        let row_y = self.line_map.y_range(row..row + 1).start;
        let text_height = self.line_map.line_text_height(row);
        let bounds_origin = self
            .last_bounds
            .map(|bounds| point(bounds.left(), bounds.top()))
            .unwrap_or_else(|| point(px(0.0), px(0.0)));
        point(
            bounds_origin.x + px(TEXT_LEFT_PAD) + self.row_layout_x(row) + self.first_text_x(row),
            bounds_origin.y + px(self.top_pad) + row_y + text_height + px(ANNOTATION_HEIGHT + 3.0),
        )
    }
}
