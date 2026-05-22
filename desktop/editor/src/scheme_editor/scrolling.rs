use super::*;

impl SchemeEditor {
    pub(super) fn reset_cursor_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_blink_state = true;
        cx.notify();

        let task = cx.spawn(
            async move |editor: gpui::WeakEntity<SchemeEditor>, cx: &mut gpui::AsyncApp| {
                cx.background_executor().timer(CURSOR_BLINK_DELAY).await;
                loop {
                    let should_continue = editor
                        .update(cx, |editor, cx| {
                            editor.cursor_blink_state = !editor.cursor_blink_state;
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

    pub fn scroll_to_cursor(&mut self, cx: &mut Context<Self>) {
        self.pending_scroll_to_cursor = true;
        let Some(scrolled) = self.try_scroll_to_cursor() else {
            return;
        };
        self.pending_scroll_to_cursor = false;
        if scrolled {
            cx.notify();
        }
    }

    pub(super) fn apply_pending_scroll_to_cursor(&mut self, cx: &mut Context<Self>) {
        if self.pending_scroll_to_cursor {
            self.scroll_to_cursor(cx);
        }
    }

    pub fn needs_cursor_scroll(&self) -> bool {
        self.pending_scroll_to_cursor
    }

    pub fn suppress_pending_scroll_to_cursor(&mut self) {
        self.pending_scroll_to_cursor = false;
    }

    pub(super) fn try_scroll_to_cursor(&mut self) -> Option<bool> {
        if self.line_map_dirty || self.is_selecting {
            return None;
        }
        let cursor = self.visual_point_for_location(self.selection.head);
        let scroll_offset = self.scroll_handle.offset();
        let scroll_bounds = self.scroll_handle.bounds();
        let viewport_height = scroll_bounds.size.height;
        if viewport_height <= px(0.0) {
            return None;
        }

        let editor_content_y = self
            .last_bounds
            .map(|bounds| bounds.top() - scroll_bounds.top() - scroll_offset.y)
            .unwrap_or(px(0.0));
        let cursor_line_height = self.line_map.row_line_height(self.selection.head.row);
        let cursor_top = editor_content_y + cursor.y + px(self.top_pad);
        let cursor_bottom = cursor_top + cursor_line_height;
        let visible_top = -scroll_offset.y;
        let visible_bottom = visible_top + viewport_height;
        let margin_height = cursor_line_height * SCROLL_MARGIN_LINES;
        let max_scroll_y = self
            .scroll_handle
            .max_offset()
            .height
            .max((self.estimated_height() - viewport_height).max(px(0.0)));

        let next_offset_y = if cursor_top - margin_height < visible_top {
            let target_y = (cursor_top - margin_height).clamp(px(0.0), max_scroll_y);
            Some(-target_y)
        } else if cursor_bottom + margin_height > visible_bottom {
            let target_y =
                (cursor_bottom + margin_height - viewport_height).clamp(px(0.0), max_scroll_y);
            Some(-target_y)
        } else {
            None
        };

        let Some(next_offset_y) = next_offset_y else {
            return Some(false);
        };
        let new_offset = point(scroll_offset.x, next_offset_y);
        if new_offset == scroll_offset {
            return Some(false);
        }
        self.scroll_handle.set_offset(new_offset);
        Some(true)
    }

    pub(super) fn start_responding_to_mouse_movements(&mut self, cx: &mut Context<Self>) {
        if self.auto_scroll_task.is_some() {
            return;
        }

        let task = cx.spawn(
            async move |editor: gpui::WeakEntity<SchemeEditor>, cx: &mut gpui::AsyncApp| loop {
                cx.background_executor().timer(AUTO_SCROLL_INTERVAL).await;
                let should_continue = editor
                    .update(cx, |editor, cx| {
                        if !editor.is_selecting {
                            return false;
                        }
                        editor.auto_scroll_selection(cx);
                        true
                    })
                    .ok()
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            },
        );
        self.auto_scroll_task = Some(task);
    }

    pub(super) fn stop_responding_to_mouse_movements(&mut self) {
        self.auto_scroll_task = None;
        self.auto_scroll_last_mouse_position = None;
    }

    pub(super) fn auto_scroll_selection(&mut self, cx: &mut Context<Self>) {
        let Some(mouse_pos) = self.auto_scroll_last_mouse_position else {
            return;
        };

        self.update_mouse_selection_to_position(mouse_pos);
        self.marked_range = None;

        let scroll_bounds = self.scroll_handle.bounds();
        let viewport_top = scroll_bounds.top();
        let viewport_bottom = scroll_bounds.bottom();
        let distance_above =
            (viewport_top - mouse_pos.y - px(AUTO_SCROLL_MIN_THRESHOLD)).max(px(0.0));
        let distance_below =
            (mouse_pos.y - viewport_bottom - px(AUTO_SCROLL_MIN_THRESHOLD)).max(px(0.0));

        if distance_above > px(0.0) || distance_below > px(0.0) {
            let scroll_offset = self.scroll_handle.offset();
            let max_scroll_y = self
                .scroll_handle
                .max_offset()
                .height
                .max((self.estimated_height() - scroll_bounds.size.height).max(px(0.0)));
            let distance = distance_above.max(distance_below);
            let t =
                (distance / (AUTO_SCROLL_MAX_THRESHOLD - AUTO_SCROLL_MIN_THRESHOLD)).min(px(1.0));
            let scroll_speed = px(t.to_f64() as f32) * AUTO_SCROLL_MAX_SPEED;
            let new_scroll_y = if distance_above > px(0.0) {
                (scroll_offset.y + scroll_speed).clamp(-max_scroll_y, px(0.0))
            } else {
                (scroll_offset.y - scroll_speed).clamp(-max_scroll_y, px(0.0))
            };
            self.scroll_handle
                .set_offset(point(scroll_offset.x, new_scroll_y));
        }

        self.cursor_blink_state = true;
        cx.emit(EditorEvent::SelectionChanged {
            scheme_id: self.scheme_id,
        });
        cx.notify();
    }
}
