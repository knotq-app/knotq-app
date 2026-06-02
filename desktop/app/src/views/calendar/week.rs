use gpui::{ScrollDelta, ScrollHandle, ScrollWheelEvent, TouchPhase};

use super::*;

const CALENDAR_SWIPE_DOMINANCE: f32 = 1.2;
const CALENDAR_SWIPE_TRIGGER_RATIO: f32 = 0.35;
const CALENDAR_SWIPE_MAX_RATIO: f32 = 0.9;

/// Convert a window-relative Y coordinate to an hour fraction (0.0–24.0)
/// using the calendar scroll container's bounds and offset.
pub(super) fn window_y_to_hour(window_y: f32, scroll_handle: &ScrollHandle) -> f32 {
    let bounds = scroll_handle.bounds();
    let viewport_y = window_y - f32::from(bounds.top());
    let scroll_offset_y = f32::from(scroll_handle.offset().y);
    let content_y = viewport_y - scroll_offset_y;
    ((content_y - TIME_Y_OFFSET) / HOUR_H).clamp(0.0, 24.0)
}

/// Resolve the target day for a move gesture from the cursor's horizontal
/// displacement relative to where the drag began. Using `round()` on the
/// displacement gives a half-column deadzone, so a mostly-vertical drag with a
/// little sideways wobble keeps the original day instead of randomly jumping to
/// an adjacent one. The result is clamped to the visible week.
pub(super) fn move_day_for_x(
    grab_x: f32,
    original_date: chrono::NaiveDate,
    window_x: f32,
    day_col_w: f32,
    visible_start: chrono::NaiveDate,
    visible_days: usize,
) -> chrono::NaiveDate {
    let day_delta = ((window_x - grab_x) / day_col_w.max(1.0)).round() as i64;
    let date = original_date + Duration::days(day_delta);
    let last = visible_start + Duration::days(visible_days as i64 - 1);
    date.clamp(visible_start, last)
}

fn move_preview_hour(hour: f32) -> f32 {
    hour.clamp(0.0, 24.0)
}

fn calendar_scroll_delta_px(event: &ScrollWheelEvent) -> (f32, f32) {
    match event.delta {
        ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
        ScrollDelta::Lines(delta) => (delta.x * HOUR_H, delta.y * HOUR_H),
    }
}

fn horizontal_calendar_swipe_delta(event: &ScrollWheelEvent, active_offset_x: f32) -> Option<f32> {
    let (dx, dy) = calendar_scroll_delta_px(event);
    if active_offset_x.abs() > f32::EPSILON {
        return Some(dx);
    }
    if dx.abs() > 2.0 && dx.abs() >= dy.abs() * CALENDAR_SWIPE_DOMINANCE {
        Some(dx)
    } else {
        None
    }
}

fn calendar_swipe_period_delta(offset_x: f32, day_col_w: f32) -> i32 {
    let threshold = day_col_w * CALENDAR_SWIPE_TRIGGER_RATIO;
    if offset_x <= -threshold {
        1
    } else if offset_x >= threshold {
        -1
    } else {
        0
    }
}

/// Snap an absolute hour to the same 15-minute grid `snapped_calendar_datetime`
/// uses, so resize/create ghosts preview exactly where the edge will land. Keep
/// in sync with `knotq_date_util::snapped_calendar_datetime`.
pub(super) fn snap_preview_hour(hour: f32) -> f32 {
    let minutes = (hour * 60.0).round() as i64;
    let snapped = ((minutes + 7) / 15 * 15).clamp(0, 24 * 60);
    snapped as f32 / 60.0
}

impl KnotQApp {
    pub(super) fn render_week_calendar(
        &mut self,
        available_width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme();
        // Default the calendar to scrolling to the bottom (end-of-day hours visible)
        // on first render. After that, leave the user's scroll position alone.
        if !self.cal_scroll_initialized {
            self.cal_scroll_handle.scroll_to_bottom();
            self.cal_scroll_initialized = true;
        }
        const TIME_W: f32 = 50.0;
        const TIME_LABEL_H: f32 = 12.0;
        const HEADER_H: f32 = 40.0;

        let today = Local::now().date_naive();
        let week_start = self.calendar_week_start();
        let (visible_start, visible_days) =
            visible_week_range(week_start, today, available_width, TIME_W);
        let day_col_w =
            ((available_width - TIME_W).max(MIN_WEEK_DAY_W) / visible_days as f32).max(1.0);
        let swipe_x = self
            .cal_swipe
            .offset_x
            .clamp(-day_col_w * CALENDAR_SWIPE_MAX_RATIO, day_col_w * CALENDAR_SWIPE_MAX_RATIO);

        // Materialize all occurrences for the week.
        let week_start_utc = Local
            .with_ymd_and_hms(
                week_start.year(),
                week_start.month(),
                week_start.day(),
                0,
                0,
                0,
            )
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let week_end_utc = week_start_utc + Duration::days(7);
        self.ensure_daily_queue_calendar_range_loaded(
            week_start,
            week_start + Duration::days(6),
            cx,
        );

        let all_tasks = self.collect_calendar_tasks(week_start_utc, week_end_utc);

        let day_h = HOUR_H * 24.0;
        let total_h = TIME_Y_OFFSET + day_h;
        let now_local = Local::now();
        let block_stroke = token_rgba(t.cal_event_text);

        // Day headers
        let mut day_cells: Vec<gpui::AnyElement> = Vec::new();
        for i in 0..visible_days {
            let date = visible_start + Duration::days(i as i64);
            let is_today = date == today;
            let weekday = match date.weekday() {
                Weekday::Mon => "Mon",
                Weekday::Tue => "Tue",
                Weekday::Wed => "Wed",
                Weekday::Thu => "Thu",
                Weekday::Fri => "Fri",
                Weekday::Sat => "Sat",
                Weekday::Sun => "Sun",
            };
            let month_label = i == 0 || date.day() == 1;
            let lead = if month_label {
                date.format("%B").to_string()
            } else {
                weekday.to_string()
            };
            let label: gpui::AnyElement = div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .text_size(px(15.0))
                .font_family(FONT_DISPLAY)
                .font_weight(if is_today || month_label {
                    gpui::FontWeight::BOLD
                } else {
                    gpui::FontWeight::MEDIUM
                })
                .text_color(token_hsla(if is_today {
                    t.text_today
                } else {
                    t.text_primary
                }))
                .child(lead)
                .child(date.day().to_string())
                .into_any_element();

            day_cells.push(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(label)
                    .into_any_element(),
            );
        }

        let header = div()
            .flex()
            .w_full()
            .h(px(HEADER_H))
            .bg(token_hsla(t.bg_app))
            .border_b_1()
            .border_color(token_rgba(t.divider_tiny))
            .child(
                div()
                    .w(px(TIME_W))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(5.0))
                    .child(
                        div()
                            .id("cal-prev")
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(token_hsla(t.text_muted))
                            .hover({
                                let c = t.text_primary;
                                move |s| s.text_color(token_hsla(c))
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.shift_calendar_period(-1);
                                cx.notify();
                            }))
                            .child("‹"),
                    )
                    .child(
                        div()
                            .id("cal-tod")
                            .cursor_pointer()
                            .text_size(px(10.0))
                            .text_color(token_hsla(t.text_muted))
                            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.reset_calendar_period();
                                cx.notify();
                            }))
                            .child("·"),
                    )
                    .child(
                        div()
                            .id("cal-next")
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(token_hsla(t.text_muted))
                            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.shift_calendar_period(1);
                                cx.notify();
                            }))
                            .child("›"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .relative()
                            .left(px(swipe_x))
                            .flex()
                            .h_full()
                            .w_full()
                            .children(day_cells),
                    ),
            );

        // Hour grid
        let mut time_labels: Vec<gpui::AnyElement> = Vec::new();
        for h in 0..24u32 {
            let y = TIME_Y_OFFSET + h as f32 * HOUR_H - TIME_LABEL_H / 2.0;
            time_labels.push(
                div()
                    .absolute()
                    .top(px(y))
                    .right(px(6.0))
                    .text_size(px(10.0))
                    .line_height(px(TIME_LABEL_H))
                    .font_family(FONT_MONO)
                    .text_color(token_hsla(t.text_muted))
                    .child(format_hour_label(self.time_format, h))
                    .into_any_element(),
            );
        }

        // Day columns
        let mut day_cols: Vec<gpui::AnyElement> = Vec::new();
        for col in 0..visible_days {
            let date = visible_start + Duration::days(col as i64);
            let is_past = date < today;
            let is_today = date == today;
            let is_weekend = matches!(date.weekday(), Weekday::Sat | Weekday::Sun);

            let col_tasks: Vec<&CalendarTask> = all_tasks
                .iter()
                .filter(|tk| {
                    let check = tk.start.or(tk.end);
                    check.is_some_and(|dt| dt.date_naive() == date)
                })
                .collect();

            let mut els: Vec<gpui::AnyElement> = Vec::new();

            if is_weekend {
                els.push(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(token_rgba(t.cal_weekend_tint))
                        .into_any_element(),
                );
            }

            let now_line_y: Option<f32> = if is_past {
                els.push(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(token_rgba(t.cal_past))
                        .into_any_element(),
                );
                None
            } else if is_today {
                let frac = (now_local.hour() as f32 + now_local.minute() as f32 / 60.0) / 24.0;
                let now_y = TIME_Y_OFFSET + frac * day_h;
                els.push(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(now_y))
                        .bg(token_rgba(t.cal_past))
                        .into_any_element(),
                );
                Some(now_y)
            } else {
                None
            };

            for h in 0..=24u32 {
                let raw_y = TIME_Y_OFFSET + h as f32 * HOUR_H;
                let y = if h == 24 { raw_y - 1.0 } else { raw_y };
                els.push(
                    div()
                        .absolute()
                        .top(px(y))
                        .left_0()
                        .right_0()
                        .h(px(1.0))
                        .bg(token_rgba(t.divider))
                        .into_any_element(),
                );
            }

            els.push(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(1.0))
                    .bg(token_rgba(t.border_main))
                    .into_any_element(),
            );
            if col == visible_days - 1 {
                els.push(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(px(1.0))
                        .bg(token_rgba(t.border_main))
                        .into_any_element(),
                );
            }

            // Build chunks for each type, processing in order so later types
            // can cascade horizontally to avoid prior types (ported from
            // knotqv1 CalendarView.swift).
            let events_in_col: Vec<&CalendarTask> = col_tasks
                .iter()
                .filter(|tk| tk.kind == ItemKind::Event)
                .copied()
                .collect();
            let reminders_in_col: Vec<&CalendarTask> = col_tasks
                .iter()
                .filter(|tk| tk.kind == ItemKind::Reminder)
                .copied()
                .collect();
            let assignments_in_col: Vec<&CalendarTask> = col_tasks
                .iter()
                .filter(|tk| tk.kind == ItemKind::Assignment)
                .copied()
                .collect();

            let mut prior: Vec<Vec<ScheduleChunk>> = Vec::new();
            let event_chunks = build_chunks_for_kind(&events_in_col, &prior);
            prior.push(event_chunks.clone());
            let reminder_chunks = build_chunks_for_kind(&reminders_in_col, &prior);
            prior.push(reminder_chunks.clone());
            let assignment_chunks = build_chunks_for_kind(&assignments_in_col, &prior);
            let pill_render = PillChunkRender {
                t,
                block_stroke,
                time_format: self.time_format,
                col,
                day_col_w,
            };

            let col_scroll_handle = &self.cal_scroll_handle;
            let mut event_render_order = event_chunks.iter().enumerate().collect::<Vec<_>>();
            event_render_order.sort_by_key(|(_, chunk)| {
                (
                    chunk.lane,
                    chunk.equal_groups[0][0].start.unwrap(),
                    chunk.equal_groups[0][0].end.unwrap(),
                )
            });
            for (idx, chunk) in event_render_order {
                els.push(render_event_chunk(
                    chunk,
                    t,
                    block_stroke,
                    self.time_format,
                    col,
                    idx,
                    day_col_w,
                    col_scroll_handle,
                    cx,
                ));
            }
            for (idx, chunk) in reminder_chunks.iter().enumerate() {
                els.push(render_pill_chunk(
                    chunk,
                    pill_render,
                    idx,
                    true,
                    date,
                    col_scroll_handle,
                    cx,
                ));
            }
            for (idx, chunk) in assignment_chunks.iter().enumerate() {
                els.push(render_pill_chunk(
                    chunk,
                    pill_render,
                    idx,
                    false,
                    date,
                    col_scroll_handle,
                    cx,
                ));
            }

            if let Some(now_y) = now_line_y {
                let now_color = token_rgba(t.text_today);
                els.push(
                    div()
                        .absolute()
                        .top(px(now_y - 1.25))
                        .left_0()
                        .right_0()
                        .h(px(2.5))
                        .bg(now_color)
                        .into_any_element(),
                );
                els.push(
                    div()
                        .absolute()
                        .top(px(now_y - 4.0))
                        .left(px(-4.0))
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(now_color)
                        .into_any_element(),
                );
            }

            // Ghost preview for dragging an existing item to reschedule. The
            // preview is anchored to the *exact* dates the move will commit
            // (`draft_dates`), so what the user sees while dragging is precisely
            // where the item lands — same day, same time, no separate math.
            if let Some(mv) = &self.cal_move {
                if !mv.is_negligible() {
                    let (draft_start, draft_end) = mv.draft_dates();
                    let start_local = draft_start.map(|d| d.with_timezone(&Local));
                    let end_local = draft_end.map(|d| d.with_timezone(&Local));
                    // The block lives on its start's day (or end's, for an
                    // assignment); only draw it in that column.
                    let anchor_local = start_local.or(end_local);
                    if anchor_local.map(|d| d.date_naive()) == Some(date) {
                        let hour_of = |dt: chrono::DateTime<Local>| {
                            dt.hour() as f32 + dt.minute() as f32 / 60.0
                        };
                        let start_h = start_local.map(hour_of);
                        let end_h = end_local.map(|e| match start_local {
                            // End rolled past midnight relative to start — clamp
                            // the visible bottom to the end of this day.
                            Some(s) if s.date_naive() != e.date_naive() => 24.0,
                            _ => hour_of(e),
                        });
                        let lo = move_preview_hour(start_h.or(end_h).unwrap());
                        let hi = move_preview_hour(end_h.or(start_h).unwrap());
                        let lo_y = TIME_Y_OFFSET + lo.min(hi) * HOUR_H;
                        let hi_y = TIME_Y_OFFSET + lo.max(hi) * HOUR_H;
                        let ghost_h = (hi_y - lo_y).max(8.0);
                        els.push(
                            div()
                                .absolute()
                                .top(px(lo_y))
                                .left(px(2.0))
                                .right(px(2.0))
                                .h(px(ghost_h))
                                .rounded(px(3.0))
                                .bg(token_rgba(t.cal_event_text))
                                .opacity(0.25)
                                .into_any_element(),
                        );
                    }
                }
            }

            // Ghost preview for dragging the bottom edge of an event.
            if let Some(resize) = &self.cal_resize {
                if resize.date == date {
                    let snapped_edge = move_preview_hour(snap_preview_hour(resize.current_hour));
                    let lo = resize.original_start_hour.min(snapped_edge);
                    let hi = resize.original_start_hour.max(snapped_edge);
                    let lo_y = TIME_Y_OFFSET + lo * HOUR_H;
                    let hi_y = TIME_Y_OFFSET + hi * HOUR_H;
                    let ghost_h = (hi_y - lo_y).max(4.0);
                    els.push(
                        div()
                            .absolute()
                            .top(px(lo_y))
                            .left(px(2.0))
                            .right(px(2.0))
                            .h(px(ghost_h))
                            .rounded(px(3.0))
                            .bg(token_rgba(t.cal_event_text))
                            .opacity(0.22)
                            .into_any_element(),
                    );
                }
            }

            // Ghost preview block while dragging to create.
            if let Some(drag) = &self.cal_drag {
                if drag.date == date && drag.is_dragging {
                    let snapped_start = snap_preview_hour(drag.start_hour);
                    let snapped_current = snap_preview_hour(drag.current_hour);
                    let lo_y = TIME_Y_OFFSET
                        + move_preview_hour(snapped_start.min(snapped_current)) * HOUR_H;
                    let hi_y = TIME_Y_OFFSET
                        + move_preview_hour(snapped_start.max(snapped_current)) * HOUR_H;
                    let drag_h = (hi_y - lo_y).max(4.0);
                    els.push(
                        div()
                            .absolute()
                            .top(px(lo_y))
                            .left(px(2.0))
                            .right(px(2.0))
                            .h(px(drag_h))
                            .rounded(px(3.0))
                            .bg(token_rgba(t.cal_event_text))
                            .opacity(0.18)
                            .into_any_element(),
                    );
                }
            }

            // Capture the scroll handle for the mouse-down closure to compute
            // the column-local Y from window-relative mouse coordinates.
            let scroll_handle = self.cal_scroll_handle.clone();
            let show_create_cursor = self
                .cal_drag
                .as_ref()
                .is_some_and(|drag| drag.date == date && drag.is_dragging);

            day_cols.push(
                div()
                    .id(("cal-day", col))
                    .flex_1()
                    .relative()
                    .h(px(total_h))
                    .when(show_create_cursor, |column| column.cursor_crosshair())
                    .on_mouse_down(MouseButton::Left, {
                        let scroll_handle = scroll_handle.clone();
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            if this.event_popup.is_some() {
                                return;
                            }
                            let hour =
                                window_y_to_hour(f32::from(event.position.y), &scroll_handle);
                            this.cal_drag = Some(CalendarDragState {
                                date,
                                start_hour: hour,
                                current_hour: hour,
                                is_dragging: false,
                                shift: event.modifiers.shift,
                            });
                            cx.notify();
                        })
                    })
                    .on_mouse_move({
                        let scroll_handle = scroll_handle.clone();
                        cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                            if !event.dragging() {
                                this.clear_calendar_pointer_state(cx);
                                return;
                            }
                            let hour =
                                window_y_to_hour(f32::from(event.position.y), &scroll_handle);
                            if let Some(drag) = this.cal_drag.as_mut() {
                                if drag.date == date {
                                    drag.current_hour = hour;
                                    drag.is_dragging =
                                        drag.is_dragging || (hour - drag.start_hour).abs() > 0.125;
                                    cx.notify();
                                }
                            }
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
                            }
                            if let Some(resize) = this.cal_resize.as_mut() {
                                if resize.date == date {
                                    resize.current_hour = hour;
                                    resize.anchor = event.position;
                                    cx.notify();
                                }
                            }
                        })
                    })
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            if let Some(mut resize) = this.cal_resize.take() {
                                resize.anchor = event.position;
                                this.commit_calendar_resize(resize, cx);
                                cx.notify();
                                return;
                            }
                            if let Some(mut mv) = this.cal_move.take() {
                                mv.date = move_day_for_x(
                                    mv.grab_x,
                                    mv.original_date,
                                    f32::from(event.position.x),
                                    day_col_w,
                                    visible_start,
                                    visible_days,
                                );
                                mv.anchor = event.position;
                                this.commit_calendar_move(mv, cx);
                                cx.notify();
                                return;
                            }
                            if let Some(drag) = this.cal_drag.take() {
                                this.create_calendar_item_from_drag(
                                    drag.date,
                                    drag.start_hour,
                                    drag.current_hour,
                                    drag.shift,
                                    event.position,
                                    window,
                                    cx,
                                );
                                cx.notify();
                            }
                        }),
                    )
                    .children(els)
                    .into_any_element(),
            );
        }

        let grid_body = div()
            .flex_1()
            .flex()
            .w_full()
            .child(
                div()
                    .w(px(TIME_W))
                    .h(px(total_h))
                    .flex_shrink_0()
                    .relative()
                    .children(time_labels),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .relative()
                            .left(px(swipe_x))
                            .flex()
                            .w_full()
                            .children(day_cols),
                    ),
            );

        // While a calendar gesture is in flight, a single transparent overlay
        // covering the whole grid captures every pointer move/release. GPUI only
        // delivers mouse events to the topmost hitbox, so the per-column/per-block
        // handlers stop firing once the cursor is over another block; the overlay
        // guarantees the ghost tracks the cursor and that the gesture always ends
        // (committing or opening the popup) regardless of what is underneath.
        let drag_overlay = self.calendar_gesture_overlay(
            visible_start,
            visible_days,
            day_col_w,
            TIME_W,
            total_h,
            cx,
        );

        div()
            .flex_1()
            .flex()
            .flex_col()
            .h_full()
            .bg(token_hsla(t.bg_app))
            .text_color(token_hsla(t.text_primary))
            .on_scroll_wheel(cx.listener(
                move |this, event: &ScrollWheelEvent, _window, cx| {
                    this.handle_calendar_horizontal_swipe(event, day_col_w, cx);
                },
            ))
            .child(header)
            .child(
                div()
                    .id("cal-scroll")
                    .track_scroll(&self.cal_scroll_handle)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .relative()
                            .flex()
                            .w_full()
                            .child(grid_body)
                            .children(drag_overlay),
                    ),
            )
            .into_any_element()
    }

    /// Build the full-grid capture overlay used while a move/resize/create
    /// gesture is active. Returns `None` when no gesture is in flight.
    #[allow(clippy::too_many_arguments)]
    fn calendar_gesture_overlay(
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
                            drag.date,
                            drag.start_hour,
                            drag.current_hour,
                            drag.shift,
                            pos,
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

    fn handle_calendar_horizontal_swipe(
        &mut self,
        event: &ScrollWheelEvent,
        day_col_w: f32,
        cx: &mut Context<Self>,
    ) {
        let active_offset = self.cal_swipe.offset_x;
        let Some(delta_x) = horizontal_calendar_swipe_delta(event, active_offset) else {
            return;
        };

        if matches!(event.touch_phase, TouchPhase::Started) {
            self.cal_swipe.offset_x = 0.0;
        }

        let max_offset = day_col_w * CALENDAR_SWIPE_MAX_RATIO;
        self.cal_swipe.offset_x = (self.cal_swipe.offset_x + delta_x).clamp(-max_offset, max_offset);

        if !event.delta.precise() || matches!(event.touch_phase, TouchPhase::Ended) {
            let period_delta = calendar_swipe_period_delta(self.cal_swipe.offset_x, day_col_w);
            self.cal_swipe.offset_x = 0.0;
            if period_delta != 0 {
                self.shift_calendar_period(period_delta);
            }
        }

        cx.stop_propagation();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    const COL_W: f32 = 100.0;

    fn day_for(grab_x: f32, window_x: f32) -> NaiveDate {
        let start = NaiveDate::from_ymd_opt(2026, 5, 25).unwrap(); // Mon
        let original = NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(); // Wed
        move_day_for_x(grab_x, original, window_x, COL_W, start, 7)
    }

    #[test]
    fn small_horizontal_wobble_keeps_the_original_day() {
        let original = NaiveDate::from_ymd_opt(2026, 5, 27).unwrap();
        // Anywhere within half a column of the grab point stays put.
        assert_eq!(day_for(350.0, 350.0), original);
        assert_eq!(day_for(350.0, 390.0), original); // +0.4 col
        assert_eq!(day_for(350.0, 310.0), original); // -0.4 col
    }

    #[test]
    fn moving_past_half_a_column_changes_the_day() {
        assert_eq!(
            day_for(350.0, 460.0), // +1.1 col → next day
            NaiveDate::from_ymd_opt(2026, 5, 28).unwrap()
        );
        assert_eq!(
            day_for(350.0, 240.0), // -1.1 col → previous day
            NaiveDate::from_ymd_opt(2026, 5, 26).unwrap()
        );
    }

    #[test]
    fn target_day_is_clamped_to_the_visible_week() {
        // Drag far left/right cannot escape the visible range.
        assert_eq!(
            day_for(350.0, -10_000.0),
            NaiveDate::from_ymd_opt(2026, 5, 25).unwrap()
        );
        assert_eq!(
            day_for(350.0, 10_000.0),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap()
        );
    }

    #[test]
    fn horizontal_swipe_threshold_selects_calendar_period() {
        assert_eq!(calendar_swipe_period_delta(-34.0, 100.0), 0);
        assert_eq!(calendar_swipe_period_delta(-35.0, 100.0), 1);
        assert_eq!(calendar_swipe_period_delta(35.0, 100.0), -1);
    }
}
