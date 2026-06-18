use super::*;

impl SchemeEditor {
    pub(super) fn select_word_at(&mut self, loc: TextLocation) -> Range<usize> {
        let offset = self.location_to_offset(loc);
        let range = word_range_at(&self.text, offset);
        self.selection = TextSelection {
            anchor: self.offset_to_location(range.start),
            head: self.offset_to_location(range.end),
        };
        range
    }

    pub(super) fn select_line_at(&mut self, loc: TextLocation) {
        if let Some(range) = self.line_range(loc.row) {
            self.selection = TextSelection {
                anchor: TextLocation {
                    row: loc.row,
                    col: 0,
                },
                head: self.offset_to_location(range.end),
            };
        }
    }

    pub(super) fn clamp_to_anchor_region(
        &self,
        anchor: TextLocation,
        loc: TextLocation,
    ) -> TextLocation {
        if let Some((first, last)) = self.cell_line_span(anchor.row) {
            if loc.row < first {
                return TextLocation { row: first, col: 0 };
            }
            if loc.row > last {
                return TextLocation {
                    row: last,
                    col: self.line_len(last),
                };
            }
            return loc;
        }

        if self.rows.get(loc.row).is_some_and(|row| row.path.is_doc()) {
            return loc;
        }

        let mut row = loc.row.min(self.rows.len().saturating_sub(1));
        if loc.row > anchor.row {
            while row > anchor.row && !self.rows.get(row).is_some_and(|row| row.path.is_doc()) {
                row -= 1;
            }
        } else {
            while row < anchor.row && !self.rows.get(row).is_some_and(|row| row.path.is_doc()) {
                row += 1;
            }
        }

        TextLocation {
            row,
            col: loc.col.min(self.line_len(row)),
        }
    }

    pub(super) fn update_mouse_selection_to_position(&mut self, position: Point<Pixels>) {
        let loc = self.clamp_to_anchor_region(
            self.selection.anchor,
            self.location_for_window_position(position),
        );
        match self.mouse_selection_mode {
            Some(MouseSelectionMode::Word {
                anchor_start,
                anchor_end,
            }) => {
                let (anchor, head) = word_drag_offsets(
                    &self.text,
                    anchor_start..anchor_end,
                    self.location_to_offset(loc),
                );
                self.selection = TextSelection {
                    anchor: self.offset_to_location(anchor),
                    head: self.offset_to_location(head),
                };
            }
            Some(MouseSelectionMode::Line { anchor_row }) => {
                let row = loc.row.min(self.render_line_count().saturating_sub(1));
                let start_row = anchor_row.min(row);
                let end_row = anchor_row.max(row);
                let start = TextLocation {
                    row: start_row,
                    col: 0,
                };
                let end = TextLocation {
                    row: end_row,
                    col: self.line_len(end_row),
                };
                self.selection = if row < anchor_row {
                    TextSelection {
                        anchor: end,
                        head: start,
                    }
                } else {
                    TextSelection {
                        anchor: start,
                        head: end,
                    }
                };
            }
            Some(MouseSelectionMode::Character) | None => {
                self.selection.head = loc;
            }
        }
    }

    pub(super) fn drag_to_position(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.is_selecting {
            return;
        }

        if let Some(origin) = self.mouse_selection_origin {
            if !mouse_moved_past_selection_epsilon(origin, position) {
                return;
            }
            self.mouse_selection_origin = None;
            self.start_responding_to_mouse_movements(cx);
        }

        self.auto_scroll_last_mouse_position = Some(position);
        self.update_mouse_selection_to_position(position);
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        cx.emit(EditorEvent::SelectionChanged {
            scheme_id: self.scheme_id,
        });
        cx.notify();
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        self.editor_focused = true;
        self.focus_handle.focus(window);

        if event.button == MouseButton::Left {
            if let Some(hitbox) = self.table_control_at(event.position) {
                self.apply_table_control(hitbox, window, cx);
                self.is_selecting = false;
                self.mouse_selection_mode = None;
                self.mouse_selection_origin = None;
                self.stop_responding_to_mouse_movements();
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }

        if event.button == MouseButton::Right {
            let loc = self.location_for_window_position(event.position);
            if let Some(row) = self.rows.get(loc.row) {
                let table = self.table_context_at_position(event.position);
                self.selection = TextSelection::collapsed(loc);
                self.mouse_selection_mode = None;
                self.mouse_selection_origin = None;
                cx.emit(EditorEvent::OpenContextMenu {
                    scheme_id: self.scheme_id,
                    item_id: table
                        .map(|context| context.table_item_id)
                        .unwrap_or(row.item.id),
                    position: event.position,
                    date_anchor: self.date_anchor_for_row(loc.row),
                    table,
                });
                self.is_selecting = false;
                self.stop_responding_to_mouse_movements();
                self.reset_cursor_blink(cx);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }

        if let Some(hitbox) = self
            .date_annotation_hitboxes
            .iter()
            .copied()
            .find(|hitbox| bounds_contains(hitbox.bounds, event.position))
        {
            cx.emit(EditorEvent::OpenDatePicker {
                scheme_id: self.scheme_id,
                item_id: hitbox.item_id,
                kind: hitbox.kind,
                anchor: point(hitbox.bounds.left(), hitbox.bounds.bottom() + px(4.0)),
            });
            self.is_selecting = false;
            self.mouse_selection_mode = None;
            self.mouse_selection_origin = None;
            self.stop_responding_to_mouse_movements();
            self.reset_cursor_blink(cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if let Some(hitbox) = self
            .repeat_annotation_hitboxes
            .iter()
            .copied()
            .find(|hitbox| bounds_contains(hitbox.bounds, event.position))
        {
            cx.emit(EditorEvent::OpenRepeatPopover {
                scheme_id: self.scheme_id,
                item_id: hitbox.item_id,
                anchor: point(hitbox.bounds.left(), hitbox.bounds.bottom() + px(4.0)),
            });
            self.is_selecting = false;
            self.mouse_selection_mode = None;
            self.mouse_selection_origin = None;
            self.stop_responding_to_mouse_movements();
            self.reset_cursor_blink(cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if let Some(hitbox) = self
            .checkbox_hitboxes
            .iter()
            .copied()
            .find(|hitbox| bounds_contains(hitbox.bounds, event.position))
        {
            cx.emit(EditorEvent::Command(Command::ToggleOccurrence {
                scheme: self.scheme_id,
                item: hitbox.item_id,
                occurrence: OccurrenceId::Single,
            }));
            self.is_selecting = false;
            self.mouse_selection_mode = None;
            self.mouse_selection_origin = None;
            self.stop_responding_to_mouse_movements();
            self.reset_cursor_blink(cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }

        self.is_selecting = true;
        self.mouse_selection_mode = Some(MouseSelectionMode::Character);
        self.mouse_selection_origin = Some(event.position);
        let loc = self.location_for_window_position(event.position);
        if event.modifiers.shift {
            self.selection.head = self.clamp_to_anchor_region(self.selection.anchor, loc);
            self.mouse_selection_mode = Some(MouseSelectionMode::Character);
        } else if event.click_count == 2 {
            let range = self.select_word_at(loc);
            self.mouse_selection_mode = Some(MouseSelectionMode::Word {
                anchor_start: range.start,
                anchor_end: range.end,
            });
        } else if event.click_count >= 3 {
            self.select_line_at(loc);
            self.mouse_selection_mode = Some(MouseSelectionMode::Line {
                anchor_row: loc.row,
            });
        } else {
            self.selection = TextSelection::collapsed(loc);
            self.mouse_selection_mode = Some(MouseSelectionMode::Character);
        }
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        cx.emit(EditorEvent::Focused {
            scheme_id: self.scheme_id,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hovered_table_control = self
            .table_control_at(event.position)
            .map(|hitbox| hitbox.kind);
        if hovered_table_control != self.hovered_table_control {
            self.hovered_table_control = hovered_table_control;
            cx.notify();
        }

        if self.is_selecting && event.dragging() {
            self.drag_to_position(event.position, cx);
            cx.stop_propagation();
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = false;
        self.mouse_selection_mode = None;
        self.mouse_selection_origin = None;
        self.stop_responding_to_mouse_movements();
        cx.notify();
    }
}

fn word_drag_offsets(
    text: &str,
    anchor_range: Range<usize>,
    hover_offset: usize,
) -> (usize, usize) {
    let hover_range = word_range_at(text, hover_offset);
    if hover_range.start < anchor_range.start {
        (anchor_range.end, hover_range.start)
    } else {
        (anchor_range.start, hover_range.end)
    }
}

fn mouse_moved_past_selection_epsilon(origin: Point<Pixels>, position: Point<Pixels>) -> bool {
    let dx = (position.x - origin.x).to_f64() as f32;
    let dy = (position.y - origin.y).to_f64() as f32;
    dx * dx + dy * dy >= MOUSE_SELECTION_DRAG_EPSILON * MOUSE_SELECTION_DRAG_EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_drag_keeps_original_word_when_pointer_stays_inside_it() {
        let text = "alpha beta gamma";

        assert_eq!(word_drag_offsets(text, 6..10, 8), (6, 10));
    }

    #[test]
    fn word_drag_expands_by_whole_words_in_drag_direction() {
        let text = "alpha beta gamma";

        assert_eq!(word_drag_offsets(text, 6..10, 2), (10, 0));
        assert_eq!(word_drag_offsets(text, 6..10, 13), (6, 16));
    }

    #[test]
    fn mouse_selection_waits_for_drag_epsilon() {
        let origin = point(px(10.0), px(10.0));

        assert!(!mouse_moved_past_selection_epsilon(
            origin,
            point(px(15.0), px(10.0))
        ));
        assert!(mouse_moved_past_selection_epsilon(
            origin,
            point(px(16.0), px(10.0))
        ));
        assert!(mouse_moved_past_selection_epsilon(
            origin,
            point(px(14.5), px(14.5))
        ));
    }
}
