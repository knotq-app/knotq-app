use super::*;

const CALENDAR_BLOCK_ALPHA_MULTIPLIER: f32 = 0.62;
const EVENT_RESIZE_HANDLE_PX: f32 = 7.0;

fn calendar_block_bg(t: Theme, offset: bool) -> gpui::Rgba {
    let mut source = token_rgba(t.event_bg);
    if offset {
        source.a = (source.a * 0.92).clamp(0.0, 1.0);
    }
    let target_alpha = (source.a * CALENDAR_BLOCK_ALPHA_MULTIPLIER).clamp(0.0, 1.0);
    preserve_effective_color(source, token_rgba(t.bg_app), target_alpha)
}

fn preserve_effective_color(fg: gpui::Rgba, bg: gpui::Rgba, alpha: f32) -> gpui::Rgba {
    if alpha <= 0.0 {
        return gpui::Rgba { a: 0.0, ..fg };
    }

    let effective = |fg_ch: f32, bg_ch: f32| fg_ch * fg.a + bg_ch * (1.0 - fg.a);
    let source = |effective_ch: f32, bg_ch: f32| {
        ((effective_ch - bg_ch * (1.0 - alpha)) / alpha).clamp(0.0, 1.0)
    };

    let effective_r = effective(fg.r, bg.r);
    let effective_g = effective(fg.g, bg.g);
    let effective_b = effective(fg.b, bg.b);

    gpui::Rgba {
        r: source(effective_r, bg.r),
        g: source(effective_g, bg.g),
        b: source(effective_b, bg.b),
        a: alpha,
    }
}

pub(super) fn render_event_chunk<'a>(
    chunk: &ScheduleChunk<'a>,
    t: Theme,
    block_stroke: gpui::Rgba,
    time_format: TimeFormat,
    col: usize,
    idx: usize,
    day_col_w: f32,
    scroll_handle: &gpui::ScrollHandle,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let head = chunk.equal_groups[0][0];
    let min_start = head.start.unwrap();
    let max_end = head.end.unwrap();

    let top = time_y(min_start);
    let day = min_start.date_naive();
    let bot_actual = time_y_clamped_end(max_end, day);
    let actual_height = (bot_actual - top).max(0.0);
    let line_count = chunk.equal_groups.iter().map(|g| g.len()).sum::<usize>();
    let span_minutes = (max_end - min_start).num_minutes();
    let hide_time = (head.kind == ItemKind::Event && span_minutes <= 30) || !chunk.show_time;
    let compact = chunk.lane > 0;
    let height = actual_height.max(event_min_height(hide_time, line_count));

    let item_bg = calendar_block_bg(t, idx % 2 == 1);
    let (block_left, block_w) = event_block_geometry(day_col_w, chunk.lane);
    let any_done = chunk
        .equal_groups
        .iter()
        .all(|g| g.iter().all(|i| i.is_done));

    let mut content: Vec<gpui::AnyElement> = Vec::new();
    let single_short_line = hide_time && line_count == 1;
    let event_title_h = if single_short_line {
        height.clamp(8.0, RUN_LINE_HOURS * HOUR_H)
    } else if hide_time {
        13.0
    } else {
        RUN_LINE_HOURS * HOUR_H
    };
    let event_title_size = if single_short_line {
        (height - 1.0).clamp(8.0, FONT_SIZE_CALENDAR_ITEM)
    } else if hide_time {
        10.0
    } else {
        FONT_SIZE_CALENDAR_ITEM
    };
    let mut content_y = if single_short_line { 0.0 } else { 3.0 };
    for group in &chunk.equal_groups {
        if !hide_time {
            let g_head = group[0];
            let time_str = format_event_time_range(
                time_format,
                g_head.start.unwrap(),
                g_head.end.unwrap(),
                compact,
            );
            let time_color = calendar_time_color(g_head.start.unwrap(), group, t);
            content.push(calendar_time_line(
                time_str, time_color, content_y, 4.0, block_w,
            ));
            content_y += TIME_HEADER_HOURS * HOUR_H;
        }
        for task in group {
            let item_color = calendar_task_color(task, t.is_dark);
            content.push(calendar_event_title_line(
                calendar_item_title(&task.text),
                item_color,
                task.is_done,
                content_y,
                6.0,
                block_w,
                event_title_h,
                event_title_size,
            ));
            content_y += event_title_h;
        }
    }
    let block_target = CalendarPopupTarget::from_task(head);
    if !block_target.is_read_only {
        content.push(calendar_event_resize_handle(block_target.clone(), day, cx));
    }
    let block_editable = !block_target.is_read_only;
    let move_target = block_target.clone();
    let click_target = block_target.clone();

    div()
        .id(("ev", col * 1000 + idx))
        .absolute()
        .top(px(top))
        .h(px(height))
        .left(px(block_left))
        .w(px(block_w))
        .rounded(px(3.0))
        .bg(item_bg)
        .border_1()
        .border_color(block_stroke)
        .overflow_hidden()
        .opacity(if any_done { 0.55 } else { 1.0 })
        .when(block_editable, |s| s.cursor_grab())
        .when(!block_editable, |s| s.cursor_pointer())
        .on_mouse_down(MouseButton::Left, {
            let item_date = day;
            let scroll_handle = scroll_handle.clone();
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                if move_target.is_read_only {
                    cx.stop_propagation();
                    return;
                }
                let grab_hour =
                    super::week::window_y_to_hour(f32::from(event.position.y), &scroll_handle);
                this.cal_move = Some(CalendarMoveState {
                    scheme_id: move_target.scheme_id,
                    item_id: move_target.item_id,
                    occurrence: move_target.occurrence.clone(),
                    occurrence_index: move_target.occurrence_index,
                    date: item_date,
                    original_date: item_date,
                    occurrence_start: move_target.start,
                    occurrence_end: move_target.end,
                    grab_hour,
                    grab_x: f32::from(event.position.x),
                    current_hour: grab_hour,
                    anchor: event.position,
                });
                cx.stop_propagation();
                cx.notify();
            })
        })
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                if let Some(mut resize) = this.cal_resize.take() {
                    resize.anchor = event.position;
                    this.commit_calendar_resize(resize, cx);
                    cx.notify();
                    return;
                }
                if let Some(mut mv) = this.cal_move.take() {
                    mv.anchor = event.position;
                    this.commit_calendar_move(mv, cx);
                    cx.notify();
                }
            }),
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.focus_app_root(window);
            this.open_event_popup(
                click_target.scheme_id,
                click_target.item_id,
                click_target.occurrence.clone(),
                click_target.occurrence_index,
                click_target.start,
                click_target.end,
                event.position(),
                false,
                false,
                window,
                cx,
            );
            cx.stop_propagation();
        }))
        .children(content)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_gpui::all_themes;
    use chrono::Duration;

    #[test]
    fn daily_calendar_time_uses_status_color() {
        let t = all_themes()[0];
        let when = Local::now() - Duration::minutes(1);
        let task = CalendarTask {
            scheme_id: SchemeId::new(),
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
            color_index: 0,
            is_daily: true,
            is_read_only: false,
            text: "daily reminder".to_string(),
            start: Some(when),
            end: None,
            kind: ItemKind::Reminder,
            is_done: false,
            occurrence_index: 0,
        };

        let default = token_hsla(t.text_highlight);
        assert_eq!(
            calendar_time_color(when, &[&task], t),
            date_status_color(when, default)
        );
    }
}

fn calendar_event_resize_handle(
    target: CalendarPopupTarget,
    date: chrono::NaiveDate,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let Some(start) = target.start else {
        return div().into_any_element();
    };
    let Some(end) = target.end else {
        return div().into_any_element();
    };
    if target.is_read_only {
        return div().into_any_element();
    }
    let start_local = start.with_timezone(&Local);
    let end_local = end.with_timezone(&Local);
    let original_start_hour = start_local.hour() as f32 + start_local.minute() as f32 / 60.0;
    let original_end_hour = end_local.hour() as f32 + end_local.minute() as f32 / 60.0;

    div()
        .id(SharedString::from(format!(
            "calendar-resize-{}-{}-{}",
            target.scheme_id,
            target.item_id,
            occurrence_key_fragment(&target.occurrence)
        )))
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(EVENT_RESIZE_HANDLE_PX))
        .cursor(CursorStyle::ResizeUpDown)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.cal_drag = None;
                this.cal_move = None;
                this.cal_resize = Some(CalendarResizeState {
                    scheme_id: target.scheme_id,
                    item_id: target.item_id,
                    occurrence: target.occurrence.clone(),
                    occurrence_index: target.occurrence_index,
                    date,
                    occurrence_start: start,
                    occurrence_end: end,
                    original_start_hour,
                    current_hour: original_end_hour,
                    anchor: event.position,
                });
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .into_any_element()
}

fn event_block_geometry(day_col_w: f32, lane: usize) -> (f32, f32) {
    let full_w = (day_col_w - 4.0).max(1.0);
    if lane == 0 {
        return (2.0, full_w);
    }
    let scale = (0.75 - (lane.saturating_sub(1) as f32 * 0.10)).max(0.55);
    let width = (full_w * scale).max(1.0);
    ((day_col_w - 2.0 - width).max(2.0), width)
}

fn event_min_height(hide_time: bool, line_count: usize) -> f32 {
    if hide_time {
        if line_count <= 1 {
            8.0
        } else {
            4.0 + 13.0 * line_count as f32
        }
    } else {
        3.0 + TIME_HEADER_HOURS * HOUR_H + RUN_LINE_HOURS * HOUR_H * line_count as f32 + 2.0
    }
}

fn format_event_time_range(
    time_format: TimeFormat,
    start: chrono::DateTime<Local>,
    end: chrono::DateTime<Local>,
    compact: bool,
) -> String {
    if compact {
        return match time_format {
            TimeFormat::TwelveHour => {
                format!("{}-{}", start.format("%-I:%M"), end.format("%-I:%M"))
            }
            TimeFormat::TwentyFourHour => {
                format!(
                    "{}-{}",
                    format_time(time_format, start),
                    format_time(time_format, end)
                )
            }
        };
    }
    match time_format {
        TimeFormat::TwelveHour => knotq_l10n::t_with(
            "calendar.event.time_range",
            &[
                ("start", &start.format("%-I:%M").to_string()),
                ("end", &end.format("%-I:%M %p").to_string()),
            ],
        ),
        TimeFormat::TwentyFourHour => knotq_l10n::t_with(
            "calendar.event.time_range",
            &[
                ("start", &format_time(time_format, start)),
                ("end", &format_time(time_format, end)),
            ],
        ),
    }
}

fn calendar_time_line(
    time_str: String,
    color: gpui::Hsla,
    top: f32,
    pad_x: f32,
    block_w: f32,
) -> gpui::AnyElement {
    let line_w = (block_w - pad_x * 2.0).max(1.0);
    div()
        .absolute()
        .left(px(pad_x))
        .top(px(top))
        .w(px(line_w))
        .h(px(TIME_HEADER_HOURS * HOUR_H))
        .text_size(px(FONT_SIZE_CALENDAR_TIME))
        .line_height(px(TIME_HEADER_HOURS * HOUR_H))
        .font_family(FONT_MONO)
        .text_color(color)
        .text_center()
        .whitespace_nowrap()
        .overflow_hidden()
        .child(time_str)
        .into_any_element()
}

fn calendar_title_line(
    title: String,
    item_color: gpui::Hsla,
    is_done: bool,
    top: f32,
    pad_x: f32,
    block_w: f32,
) -> gpui::AnyElement {
    let line_w = (block_w - pad_x * 2.0).max(1.0);
    div()
        .absolute()
        .left(px(pad_x))
        .top(px(top))
        .w(px(line_w))
        .h(px(RUN_LINE_HOURS * HOUR_H))
        .min_w_0()
        .text_size(px(FONT_SIZE_CALENDAR_ITEM))
        .line_height(px(RUN_LINE_HOURS * HOUR_H))
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(item_color)
        .text_center()
        .whitespace_normal()
        .line_clamp(1)
        .text_ellipsis()
        .opacity(if is_done { 0.86 } else { 1.0 })
        .child(title)
        .into_any_element()
}

fn calendar_event_title_line(
    title: String,
    item_color: gpui::Hsla,
    is_done: bool,
    top: f32,
    pad_x: f32,
    block_w: f32,
    line_h: f32,
    text_size: f32,
) -> gpui::AnyElement {
    let line_w = (block_w - pad_x * 2.0).max(1.0);

    div()
        .absolute()
        .left(px(pad_x))
        .top(px(top))
        .w(px(line_w))
        .h(px(line_h))
        .min_w_0()
        .text_size(px(text_size))
        .line_height(px(line_h))
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(item_color)
        .text_center()
        .whitespace_normal()
        .line_clamp(1)
        .text_ellipsis()
        .opacity(if is_done { 0.86 } else { 1.0 })
        .child(title)
        .into_any_element()
}

fn occurrence_key_fragment(occurrence: &OccurrenceId) -> String {
    match occurrence {
        OccurrenceId::Single => "single".to_string(),
        OccurrenceId::Recurring { original_start } => {
            format!("r{}", original_start.as_utc_lossy().timestamp_millis())
        }
    }
}

fn calendar_time_color(
    when: chrono::DateTime<Local>,
    group: &[&CalendarTask],
    t: Theme,
) -> gpui::Hsla {
    let default = token_hsla(t.text_highlight);
    if group.iter().all(|task| task.is_done) {
        default
    } else if group.iter().all(|task| task.kind == ItemKind::Event) {
        event_status_color(
            when,
            group.iter().filter_map(|task| task.end).max(),
            default,
        )
    } else {
        date_status_color(when, default)
    }
}

pub(super) fn calendar_task_color(task: &CalendarTask, is_dark: bool) -> gpui::Hsla {
    if task.is_daily {
        token_hsla(daily_queue_marker_color(is_dark))
    } else {
        calendar_item_color(task.is_done, task.color_index, is_dark)
    }
}

#[derive(Clone, Copy)]
pub(super) struct PillChunkRender {
    pub(super) t: Theme,
    pub(super) block_stroke: gpui::Rgba,
    pub(super) time_format: TimeFormat,
    pub(super) col: usize,
    pub(super) day_col_w: f32,
}

pub(super) fn render_pill_chunk<'a>(
    chunk: &ScheduleChunk<'a>,
    render: PillChunkRender,
    idx: usize,
    is_reminder: bool,
    date: chrono::NaiveDate,
    scroll_handle: &gpui::ScrollHandle,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let PillChunkRender {
        t,
        block_stroke,
        time_format,
        col,
        day_col_w,
    } = render;
    let head = chunk.equal_groups[0][0];
    let (top, bot) = estimate_range_y(&chunk.equal_groups, chunk.show_time, true);
    let top = top.max(TIME_Y_OFFSET);
    let height = (bot - top).max(18.0);
    let hide_time = !chunk.show_time;

    let item_bg = calendar_block_bg(t, idx % 2 == 1);
    let block_w = (day_col_w - 4.0 - chunk.offset).max(1.0);
    let all_done = chunk
        .equal_groups
        .iter()
        .all(|g| g.iter().all(|i| i.is_done));

    let stroke = if is_reminder {
        div().absolute().top_0()
    } else {
        div().absolute().bottom_0()
    }
    .left_0()
    .right_0()
    .h(px(1.5))
    .bg(block_stroke)
    .into_any_element();

    let mut content: Vec<gpui::AnyElement> = vec![stroke];
    let mut content_y = if is_reminder { 6.0 } else { 3.0 };
    for group in &chunk.equal_groups {
        if !hide_time {
            let g_head = group[0];
            let time_str = if is_reminder {
                knotq_l10n::t_with(
                    "calendar.pill.at_time",
                    &[("time", &format_time(time_format, g_head.start.unwrap()))],
                )
            } else {
                knotq_l10n::t_with(
                    "calendar.pill.due_time",
                    &[("time", &format_time(time_format, g_head.end.unwrap()))],
                )
            };
            let trigger = if is_reminder {
                g_head.start.unwrap()
            } else {
                g_head.end.unwrap()
            };
            let time_color = calendar_time_color(trigger, group, t);
            content.push(calendar_time_line(
                time_str, time_color, content_y, 3.0, block_w,
            ));
            content_y += TIME_HEADER_HOURS * HOUR_H;
        }
        for task in group {
            let item_color = calendar_task_color(task, t.is_dark);
            content.push(calendar_title_line(
                calendar_item_title(&task.text),
                item_color,
                task.is_done,
                content_y,
                8.0,
                block_w,
            ));
            content_y += RUN_LINE_HOURS * HOUR_H;
        }
    }

    let id_key = if is_reminder { "rem" } else { "asgn" };
    let block_target = CalendarPopupTarget::from_task(head);
    let move_target = block_target.clone();
    let click_target = block_target.clone();
    let block_editable = !block_target.is_read_only;

    div()
        .id((id_key, col * 1000 + idx))
        .absolute()
        .top(px(top))
        .h(px(height))
        .left(px(2.0 + chunk.offset))
        .right(px(2.0))
        .bg(item_bg)
        .overflow_hidden()
        .opacity(if all_done { 0.55 } else { 1.0 })
        .when(block_editable, |s| s.cursor_grab())
        .when(!block_editable, |s| s.cursor_pointer())
        .on_mouse_down(MouseButton::Left, {
            let scroll_handle = scroll_handle.clone();
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                if move_target.is_read_only {
                    cx.stop_propagation();
                    return;
                }
                let grab_hour =
                    super::week::window_y_to_hour(f32::from(event.position.y), &scroll_handle);
                this.cal_move = Some(CalendarMoveState {
                    scheme_id: move_target.scheme_id,
                    item_id: move_target.item_id,
                    occurrence: move_target.occurrence.clone(),
                    occurrence_index: move_target.occurrence_index,
                    date,
                    original_date: date,
                    occurrence_start: move_target.start,
                    occurrence_end: move_target.end,
                    grab_hour,
                    grab_x: f32::from(event.position.x),
                    current_hour: grab_hour,
                    anchor: event.position,
                });
                cx.stop_propagation();
                cx.notify();
            })
        })
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                if let Some(mut mv) = this.cal_move.take() {
                    mv.anchor = event.position;
                    this.commit_calendar_move(mv, cx);
                    cx.notify();
                }
            }),
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.focus_app_root(window);
            this.open_event_popup(
                click_target.scheme_id,
                click_target.item_id,
                click_target.occurrence.clone(),
                click_target.occurrence_index,
                click_target.start,
                click_target.end,
                event.position(),
                false,
                false,
                window,
                cx,
            );
            cx.stop_propagation();
        }))
        .children(content)
        .into_any_element()
}
