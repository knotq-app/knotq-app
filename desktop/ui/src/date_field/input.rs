use std::ops::Range;

use gpui::{
    ClipboardItem, Context, EntityInputHandler, KeyDownEvent, MouseDownEvent, Pixels, Point, Window,
};

use super::selection::DateFieldSelection;
use super::text::{clamp_range, sanitize_numeric_component};
use super::{
    DateComponentEvent, DateComponentField, DATE_FIELD_CURSOR_BLINK_DELAY,
    DATE_FIELD_CURSOR_BLINK_INTERVAL,
};

impl DateComponentField {
    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = DateFieldSelection {
            anchor: 0,
            head: self.value.len(),
        };
        cx.notify();
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection.ordered();
        if start == end {
            None
        } else {
            Some(self.value[start..end].to_string())
        }
    }

    pub(super) fn reset_cursor_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_blink_state = true;
        cx.notify();

        let task = cx.spawn(
            async move |field: gpui::WeakEntity<DateComponentField>, cx| {
                cx.background_executor()
                    .timer(DATE_FIELD_CURSOR_BLINK_DELAY)
                    .await;
                loop {
                    let should_continue = field
                        .update(cx, |field, cx| {
                            field.cursor_blink_state = !field.cursor_blink_state;
                            cx.notify();
                            true
                        })
                        .ok()
                        .unwrap_or(false);
                    if !should_continue {
                        break;
                    }
                    cx.background_executor()
                        .timer(DATE_FIELD_CURSOR_BLINK_INTERVAL)
                        .await;
                }
            },
        );
        self.cursor_blink_task = Some(task);
    }

    pub(super) fn replace_byte_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let range = clamp_range(range, self.value.len());
        let digits = sanitize_numeric_component(text, usize::MAX);
        if !text.is_empty() && digits.is_empty() {
            return;
        }

        let old = self.value.clone();
        let retained_len = self.value.len() - (range.end - range.start);
        let insert_len = self.max_len.saturating_sub(retained_len);
        let insert: String = digits.chars().take(insert_len).collect();
        self.value.replace_range(range.clone(), &insert);

        let cursor = range.start + insert.len();
        let filled =
            !insert.is_empty() && cursor >= self.max_len && self.value.len() >= self.max_len;
        self.selection = if filled {
            DateFieldSelection {
                anchor: 0,
                head: self.value.len(),
            }
        } else {
            DateFieldSelection::collapsed(cursor)
        };
        self.marked_range = None;
        self.reset_cursor_blink(cx);

        if self.value != old {
            cx.emit(DateComponentEvent::Change);
            if filled {
                cx.emit(DateComponentEvent::Filled);
            }
        }
        cx.notify();
    }

    pub(super) fn move_to(&mut self, offset: usize, select: bool, cx: &mut Context<Self>) {
        let offset = offset.min(self.value.len());
        if select {
            self.selection.head = offset;
        } else {
            self.selection = DateFieldSelection::collapsed(offset);
        }
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn move_left(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (start, _) = self.selection.ordered();
            self.move_to(start, false, cx);
            return;
        }
        self.move_to(self.selection.head.saturating_sub(1), select, cx);
    }

    pub(super) fn move_right(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (_, end) = self.selection.ordered();
            self.move_to(end, false, cx);
            return;
        }
        self.move_to((self.selection.head + 1).min(self.value.len()), select, cx);
    }

    pub(super) fn backspace(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
            return;
        }
        let cursor = self.selection.head;
        if cursor > 0 {
            self.replace_byte_range(cursor - 1..cursor, "", cx);
        }
    }

    pub(super) fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
            return;
        }
        let cursor = self.selection.head;
        if cursor < self.value.len() {
            self.replace_byte_range(cursor..cursor + 1, "", cx);
        }
    }

    pub(super) fn cut(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
        }
    }

    pub(super) fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, &text, cx);
        }
    }

    pub fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.platform || modifiers.control;

        if command {
            match key {
                "a" => {
                    self.select_all(cx);
                    cx.stop_propagation();
                }
                "c" => {
                    self.copy(cx);
                    cx.stop_propagation();
                }
                "x" => {
                    self.cut(cx);
                    cx.stop_propagation();
                }
                "v" => {
                    self.paste(cx);
                    cx.stop_propagation();
                }
                "z" if modifiers.shift => {
                    cx.emit(DateComponentEvent::Redo);
                    cx.stop_propagation();
                }
                "z" => {
                    cx.emit(DateComponentEvent::Undo);
                    cx.stop_propagation();
                }
                "left" => {
                    self.move_to(0, modifiers.shift, cx);
                    cx.stop_propagation();
                }
                "right" => {
                    self.move_to(self.value.len(), modifiers.shift, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }

        match key {
            "left" => {
                self.move_left(modifiers.shift, cx);
                cx.stop_propagation();
            }
            "right" => {
                self.move_right(modifiers.shift, cx);
                cx.stop_propagation();
            }
            "home" => {
                self.move_to(0, modifiers.shift, cx);
                cx.stop_propagation();
            }
            "end" => {
                self.move_to(self.value.len(), modifiers.shift, cx);
                cx.stop_propagation();
            }
            "backspace" => {
                self.backspace(cx);
                cx.stop_propagation();
            }
            "delete" => {
                self.delete_forward(cx);
                cx.stop_propagation();
            }
            "enter" => {
                cx.emit(DateComponentEvent::PressEnter);
                cx.stop_propagation();
            }
            "tab" if modifiers.shift => {
                cx.emit(DateComponentEvent::TabBackward);
                cx.stop_propagation();
            }
            "tab" => {
                cx.emit(DateComponentEvent::TabForward);
                cx.stop_propagation();
            }
            "escape" => {
                cx.emit(DateComponentEvent::Cancel);
                cx.stop_propagation();
            }
            _ => {
                self.reset_cursor_blink(cx);
            }
        }
    }

    pub fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_all_on_focus = false;
        self.focus_handle.focus(window);
        let offset = self.character_index_for_point(event.position, window, cx);
        if event.click_count > 1 {
            self.select_all(cx);
        } else if event.modifiers.shift {
            self.selection.head = offset.unwrap_or(self.selection.head).min(self.value.len());
        } else {
            self.selection = DateFieldSelection::collapsed(
                offset.unwrap_or(self.value.len()).min(self.value.len()),
            );
        }
        self.is_selecting = true;
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn drag_to_position(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(offset) = self.character_index_for_point(position, window, cx) {
            self.selection.head = offset.min(self.value.len());
            self.reset_cursor_blink(cx);
            cx.notify();
        }
    }
}
