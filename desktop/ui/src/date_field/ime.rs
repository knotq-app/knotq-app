use std::ops::Range;

use gpui::{
    point, px, size, Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window,
};

use super::paint::{date_field_index_for_x, date_field_prefix_width, date_field_text_origin};
use super::selection::DateFieldSelection;
use super::text::clamp_range;
use super::DateComponentField;

impl EntityInputHandler for DateComponentField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = clamp_range(range, self.value.len());
        adjusted_range.replace(range.clone());
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
            range: start..end,
            reversed: self.selection.reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range.clone()
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
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| {
                let (start, end) = self.selection.ordered();
                start..end
            });
        let start = range.start.min(self.value.len());
        self.replace_byte_range(range, new_text, cx);
        self.marked_range = if new_text.is_empty() {
            None
        } else {
            Some(start..self.selection.head)
        };
        if let Some(new_range) = new_selected_range {
            let new_start = (start + new_range.start).min(self.value.len());
            let new_end = (start + new_range.end).min(self.value.len());
            self.selection = DateFieldSelection {
                anchor: new_start,
                head: new_end,
            };
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = clamp_range(range, self.value.len());
        let line_height = window.line_height();
        let origin = date_field_text_origin(element_bounds, line_height);
        let start_x = date_field_prefix_width(&self.value, range.start, window);
        let end_x = date_field_prefix_width(&self.value, range.end, window).max(start_x + px(1.0));
        Some(Bounds::new(
            point(origin.x + start_x, origin.y),
            size(end_x - start_x, line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let origin = date_field_text_origin(bounds, window.line_height());
        let local_x = point.x - origin.x;
        Some(date_field_index_for_x(&self.value, local_x, window))
    }
}
