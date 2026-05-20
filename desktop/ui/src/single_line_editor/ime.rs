use std::ops::Range;

use gpui::{
    point, size, Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window,
};

use super::selection::TextSelection;
use super::text::{
    byte_offset_to_utf16, byte_range_to_utf16_range, clamp_char_boundary,
    clamp_range_to_char_boundaries, utf16_range_to_byte_range,
};
use super::SingleLineEditor;

impl EntityInputHandler for SingleLineEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = utf16_range_to_byte_range(&self.value, range);
        let range = clamp_range_to_char_boundaries(&self.value, range);
        actual_range.replace(byte_range_to_utf16_range(&self.value, range.clone()));
        Some(self.value.get(range)?.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (start, end) = self.selection.ordered();
        Some(UTF16Selection {
            range: byte_range_to_utf16_range(&self.value, start..end),
            reversed: self.selection.reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| byte_range_to_utf16_range(&self.value, range.clone()))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| utf16_range_to_byte_range(&self.value, range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| {
                let (start, end) = self.selection.ordered();
                start..end
            });
        self.replace_byte_range(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| utf16_range_to_byte_range(&self.value, range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| {
                let (start, end) = self.selection.ordered();
                start..end
            });
        let start = clamp_char_boundary(&self.value, range.start);
        self.replace_byte_range(range, new_text, cx);
        self.marked_range = if new_text.is_empty() {
            None
        } else {
            Some(start..self.selection.head)
        };
        if let Some(new_range) = new_selected_range {
            let new_range = utf16_range_to_byte_range(new_text, new_range);
            let new_start = clamp_char_boundary(&self.value, start + new_range.start);
            let new_end = clamp_char_boundary(&self.value, start + new_range.end);
            self.selection = TextSelection {
                anchor: new_start,
                head: new_end,
            };
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = utf16_range_to_byte_range(&self.value, range);
        let range = clamp_range_to_char_boundaries(&self.value, range);
        let layout = self.last_layout.as_ref()?;
        let start_x = layout.x_for_index(range.start);
        let end_x = layout.x_for_index(range.end).max(start_x + gpui::px(1.0));
        Some(Bounds::new(
            point(bounds.left() + start_x, bounds.top()),
            size(end_x - start_x, bounds.size.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.character_index_for_window_point(point)
            .map(|offset| byte_offset_to_utf16(&self.value, offset))
    }
}
