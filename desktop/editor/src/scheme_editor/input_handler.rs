use std::ops::Range;

use gpui::{point, px, Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window};

use super::selection::TextSelection;
use super::utf16::{byte_offset_to_utf16, byte_range_to_utf16_range, utf16_range_to_byte_range};
use super::{SchemeEditor, TEXT_LEFT_PAD};

impl EntityInputHandler for SchemeEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = utf16_range_to_byte_range(&self.text, range_utf16);
        adjusted_range.replace(byte_range_to_utf16_range(&self.text, range.clone()));
        Some(self.text.get(range)?.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (start, end) = self.selection_offsets();
        Some(UTF16Selection {
            range: byte_range_to_utf16_range(&self.text, start..end),
            reversed: self.selection.reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .clone()
            .map(|range| byte_range_to_utf16_range(&self.text, range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|range| utf16_range_to_byte_range(&self.text, range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| {
                let (start, end) = self.selection_offsets();
                start..end
            });
        self.replace_byte_range(range, text, None, cx);
        self.marked_range = None;
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|range| utf16_range_to_byte_range(&self.text, range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| {
                let (start, end) = self.selection_offsets();
                start..end
            });
        let start = range.start;
        self.replace_byte_range(range, new_text, Some(window), cx);
        self.marked_range = if new_text.is_empty() {
            None
        } else {
            Some(start..start + new_text.len())
        };

        if let Some(new_range_utf16) = new_selected_range_utf16 {
            let relative = utf16_range_to_byte_range(new_text, new_range_utf16);
            self.selection = TextSelection {
                anchor: self.offset_to_location(start + relative.start),
                head: self.offset_to_location(start + relative.end),
            };
            self.scroll_to_cursor(cx);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = utf16_range_to_byte_range(&self.text, range_utf16);
        let start = self.offset_to_location(range.start);
        let end = self.offset_to_location(range.end);
        let start_point = self.visual_point_for_location(start);
        let end_point = self.visual_point_for_location(end);
        Some(Bounds::from_corners(
            point(
                element_bounds.left() + px(TEXT_LEFT_PAD) + start_point.x,
                element_bounds.top() + px(self.top_pad) + start_point.y,
            ),
            point(
                element_bounds.left()
                    + px(TEXT_LEFT_PAD)
                    + end_point.x.max(start_point.x + px(1.0)),
                element_bounds.top() + px(self.top_pad) + end_point.y + self.line_map.line_height(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        pos: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let text_origin = point(
            bounds.left() + px(TEXT_LEFT_PAD),
            bounds.top() + px(self.top_pad),
        );
        let loc =
            self.location_for_local_point(point(pos.x - text_origin.x, pos.y - text_origin.y));
        Some(byte_offset_to_utf16(
            &self.text,
            self.location_to_offset(loc),
        ))
    }
}
