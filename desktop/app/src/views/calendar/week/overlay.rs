use super::super::*;
use super::geometry::{move_day_for_x, window_y_to_hour};
use crate::app::CreateCalendarItemFromDragArgs;

impl KnotQApp {
    /// Build the full-grid capture overlay used while a move/resize/create
    /// gesture is active. Returns `None` when no gesture is in flight.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn calendar_gesture_overlay(
        &self,
        visible_start: chrono::NaiveDate,
        visible_days: usize,
        day_col_w: f32,
        time_w: f32,
        total_h: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let moving = self.cal_move.is_some();
        let resizing = self.cal_resize.is_some();
        let creating = self.cal_drag.is_some();
        if !(moving || resizing || creating) {
            return None;
        }
        let scroll_handle = self.cal_scroll_handle.clone();

        let overlay = div()
            .id("cal-drag-overlay")
            .absolute()
            .top_0()
            .left(px(time_w))
            .right_0()
            .h(px(total_h))
            .when(moving, |s| s.cursor_grab())
            .when(resizing, |s| s.cursor(CursorStyle::ResizeUpDown))
            .when(creating, |s| s.cursor_crosshair())
            .on_mouse_move({
                let scroll_handle = scroll_handle.clone();
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if !event.dragging() {
                        this.clear_calendar_pointer_state(cx);
                        return;
                    }
                    let hour = window_y_to_hour(f32::from(event.position.y), &scroll_handle);
                    if let Some(mv) = this.cal_move.as_mut() {
                        mv.date = move_day_for_x(
                            mv.grab_x,
                            mv.original_date,
                            f32::from(event.position.x),
                            day_col_w,
                            visible_start,
                            visible_days,
                        );
                        mv.current_hour = hour;
                        mv.anchor = event.position;
                        cx.notify();
                    } else if let Some(resize) = this.cal_resize.as_mut() {
                        // Resizing only adjusts the bottom edge within the event's
                        // own day; keep its column fixed and track vertically.
                        resize.current_hour = hour;
                        resize.anchor = event.position;
                        cx.notify();
                    } else if let Some(drag) = this.cal_drag.as_mut() {
                        // Create-drag stays in the column it began in.
                        drag.current_hour = hour;
                        drag.is_dragging =
                            drag.is_dragging || (hour - drag.start_hour).abs() > 0.125;
                        cx.notify();
                    }
                })
            })
            .on_mouse_up(MouseButton::Left, {
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    let pos = event.position;
                    // Commit exactly the state the ghost previewed (set by the
                    // move handler above). Re-deriving the day/hour from the
                    // up-event position can flip the result at column
                    // boundaries, making the item land somewhere other than
                    // where the ghost was shown.
                    if let Some(mut resize) = this.cal_resize.take() {
                        resize.anchor = pos;
                        this.commit_calendar_resize(resize, cx);
                        cx.notify();
                        return;
                    }
                    if let Some(mut mv) = this.cal_move.take() {
                        mv.anchor = pos;
                        this.finish_calendar_move(mv, window, cx);
                        cx.notify();
                        return;
                    }
                    if let Some(drag) = this.cal_drag.take() {
                        this.create_calendar_item_from_drag(
                            CreateCalendarItemFromDragArgs {
                                date: drag.date,
                                start_hour: drag.start_hour,
                                end_hour: drag.current_hour,
                                shift: drag.shift,
                                anchor: pos,
                            },
                            window,
                            cx,
                        );
                        cx.notify();
                    }
                })
            })
            .into_any_element();
        Some(overlay)
    }
}
