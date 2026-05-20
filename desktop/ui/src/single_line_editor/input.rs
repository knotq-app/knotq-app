use std::ops::Range;

use gpui::{ClipboardItem, Context, KeyDownEvent, Pixels, Point, Window};

use super::selection::TextSelection;
use super::text::{
    clamp_char_boundary, clamp_range_to_char_boundaries, next_char_boundary, next_word_offset,
    previous_char_boundary, previous_word_offset, sanitize_input,
};
use super::{SingleLineEditor, SingleLineEditorEvent, CURSOR_BLINK_DELAY, CURSOR_BLINK_INTERVAL};

impl SingleLineEditor {
    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = TextSelection {
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
        self.cursor_visible = true;
        cx.notify();

        let task = cx.spawn(
            async move |editor: gpui::WeakEntity<SingleLineEditor>, cx| {
                cx.background_executor().timer(CURSOR_BLINK_DELAY).await;
                loop {
                    let should_continue = editor
                        .update(cx, |editor, cx| {
                            editor.cursor_visible = !editor.cursor_visible;
                            cx.notify();
                            true
                        })
                        .ok()
                        .unwrap_or(false);
                    if !should_continue {
                        break;
                    }
                    cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                }
            },
        );
        self.cursor_blink_task = Some(task);
    }

    pub(super) fn move_to(&mut self, offset: usize, select: bool, cx: &mut Context<Self>) {
        let offset = clamp_char_boundary(&self.value, offset);
        if select {
            self.selection.head = offset;
        } else {
            self.selection = TextSelection::collapsed(offset);
        }
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn move_left(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (start, _) = self.selection.ordered();
            self.move_to(start, false, cx);
            return;
        }
        self.move_to(
            previous_char_boundary(&self.value, self.selection.head),
            select,
            cx,
        );
    }

    pub(super) fn move_right(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (_, end) = self.selection.ordered();
            self.move_to(end, false, cx);
            return;
        }
        self.move_to(
            next_char_boundary(&self.value, self.selection.head),
            select,
            cx,
        );
    }

    pub(super) fn move_word_left(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (start, _) = self.selection.ordered();
            self.move_to(start, false, cx);
            return;
        }
        self.move_to(
            previous_word_offset(&self.value, self.selection.head),
            select,
            cx,
        );
    }

    pub(super) fn move_word_right(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selection.is_empty() {
            let (_, end) = self.selection.ordered();
            self.move_to(end, false, cx);
            return;
        }
        self.move_to(
            next_word_offset(&self.value, self.selection.head),
            select,
            cx,
        );
    }

    pub(super) fn replace_byte_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let range = clamp_range_to_char_boundaries(&self.value, range);
        let replacement = sanitize_input(text);
        self.value.replace_range(range.clone(), &replacement);
        let cursor = range.start + replacement.len();
        self.selection = TextSelection::collapsed(cursor);
        self.marked_range = None;
        self.reset_cursor_blink(cx);
        cx.emit(SingleLineEditorEvent::Change);
        cx.notify();
    }

    pub(super) fn backspace(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
            return;
        }
        let cursor = self.selection.head;
        if cursor > 0 {
            self.replace_byte_range(previous_char_boundary(&self.value, cursor)..cursor, "", cx);
        }
    }

    pub(super) fn backspace_word(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
            return;
        }
        let cursor = self.selection.head;
        if cursor > 0 {
            self.replace_byte_range(previous_word_offset(&self.value, cursor)..cursor, "", cx);
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
            self.replace_byte_range(cursor..next_char_boundary(&self.value, cursor), "", cx);
        }
    }

    pub(super) fn delete_word_forward(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
            return;
        }
        let cursor = self.selection.head;
        if cursor < self.value.len() {
            self.replace_byte_range(cursor..next_word_offset(&self.value, cursor), "", cx);
        }
    }

    pub(super) fn delete_to_start(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
            return;
        }
        self.replace_byte_range(0..self.selection.head, "", cx);
    }

    pub(super) fn delete_to_end(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
            return;
        }
        self.replace_byte_range(self.selection.head..self.value.len(), "", cx);
    }

    pub(super) fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn cut(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, "", cx);
        }
    }

    pub(super) fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let (start, end) = self.selection.ordered();
            self.replace_byte_range(start..end, &text, cx);
        }
    }

    pub(super) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.platform || modifiers.control;

        if command {
            match key {
                "a" => self.select_all(cx),
                "c" => self.copy(cx),
                "x" => self.cut(cx),
                "v" => self.paste(cx),
                "left" => self.move_to(0, modifiers.shift, cx),
                "right" => self.move_to(self.value.len(), modifiers.shift, cx),
                "backspace" => self.delete_to_start(cx),
                "delete" => self.delete_to_end(cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }

        if modifiers.alt {
            match key {
                "left" => self.move_word_left(modifiers.shift, cx),
                "right" => self.move_word_right(modifiers.shift, cx),
                "backspace" => self.backspace_word(cx),
                "delete" => self.delete_word_forward(cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }

        match key {
            "left" => self.move_left(modifiers.shift, cx),
            "right" => self.move_right(modifiers.shift, cx),
            "home" => self.move_to(0, modifiers.shift, cx),
            "end" => self.move_to(self.value.len(), modifiers.shift, cx),
            "backspace" => self.backspace(cx),
            "delete" => self.delete_forward(cx),
            "enter" => cx.emit(SingleLineEditorEvent::Submit),
            "escape" => cx.emit(SingleLineEditorEvent::Cancel),
            "space" if modifiers.platform && modifiers.shift => window.show_character_palette(),
            _ => {
                self.reset_cursor_blink(cx);
                return;
            }
        }
        cx.stop_propagation();
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.prevent_default();
        cx.stop_propagation();
        self.focus_handle.focus(window);

        let offset = self
            .character_index_for_window_point(event.position)
            .unwrap_or(self.value.len());
        if event.click_count > 1 {
            self.select_all(cx);
        } else if event.modifiers.shift {
            self.selection.head = offset;
        } else {
            self.selection = TextSelection::collapsed(offset);
        }

        self.marked_range = None;
        self.is_selecting = true;
        self.reset_cursor_blink(cx);
        cx.notify();
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting && event.dragging() {
            if let Some(offset) = self.character_index_for_window_point(event.position) {
                self.selection.head = offset;
                self.reset_cursor_blink(cx);
                cx.stop_propagation();
                cx.notify();
            }
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        _event: &gpui::MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = false;
        cx.notify();
    }

    pub(super) fn drag_to_position(
        &mut self,
        position: Point<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(offset) = self.character_index_for_window_point(position) {
            self.selection.head = offset;
            self.reset_cursor_blink(cx);
            cx.notify();
        }
    }

    pub(super) fn character_index_for_window_point(&self, point: Point<Pixels>) -> Option<usize> {
        let bounds = self.last_bounds?;
        let layout = self.last_layout.as_ref()?;
        let local_x = point.x - bounds.left();
        Some(clamp_char_boundary(
            &self.value,
            layout.closest_index_for_x(local_x),
        ))
    }
}
