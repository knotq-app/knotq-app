use super::*;

pub(super) fn calendar_item_title(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        "(untitled)".to_string()
    } else {
        text.to_string()
    }
}

pub(super) fn visible_week_range(
    week_start: chrono::NaiveDate,
    today: chrono::NaiveDate,
    available_width: f32,
    time_column_width: f32,
) -> (chrono::NaiveDate, usize) {
    let usable_width = (available_width - time_column_width).max(MIN_WEEK_DAY_W);
    let capacity =
        ((usable_width / MIN_WEEK_DAY_W).floor() as usize).clamp(1, CALENDAR_WEEK_VIEW_DAYS);

    let week_end = week_start + Duration::days(CALENDAR_WEEK_VIEW_DAYS as i64 - 1);
    if (week_start..=week_end).contains(&today) && capacity < CALENDAR_WEEK_VIEW_DAYS {
        let today_index = (today - week_start).num_days().clamp(0, 6) as usize;
        let days_remaining = CALENDAR_WEEK_VIEW_DAYS - today_index;
        let extra_before_today = capacity.saturating_sub(days_remaining);
        let start_offset = today_index.saturating_sub(extra_before_today);
        return (
            week_start + Duration::days(start_offset as i64),
            capacity.min(CALENDAR_WEEK_VIEW_DAYS - start_offset).max(1),
        );
    }

    (week_start, capacity)
}

pub(super) fn time_y(dt: chrono::DateTime<Local>) -> f32 {
    TIME_Y_OFFSET
        + (dt.hour() as f32 + dt.minute() as f32 / 60.0 + dt.second() as f32 / 3600.0) * HOUR_H
}

pub(super) fn time_y_clamped_end(
    dt: chrono::DateTime<Local>,
    on_same_day: chrono::NaiveDate,
) -> f32 {
    if dt.date_naive() > on_same_day {
        TIME_Y_OFFSET + 24.0 * HOUR_H
    } else {
        time_y(dt)
    }
}

pub(super) fn ranges_overlap(a: (f32, f32), b: (f32, f32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

// Estimate the on-screen y-range a chunk would consume, in pixels.
// Mirrors `estimateRange` from knotqv1 CalendarView.swift.
pub(super) fn estimate_range_y(
    groups: &[Vec<&CalendarTask>],
    show_time: bool,
    only_visual: bool,
) -> (f32, f32) {
    let base_h = BASE_HEIGHT_HOURS * HOUR_H;
    let line_h = RUN_LINE_HOURS * HOUR_H;
    let header_h = TIME_HEADER_HOURS * HOUR_H;

    let mut increase = base_h;
    for g in groups {
        if show_time {
            increase += header_h;
        }
        increase += line_h * g.len() as f32;
    }

    let kind = groups[0][0].kind;
    match kind {
        ItemKind::Event => {
            let min_start = groups.iter().map(|g| g[0].start.unwrap()).min().unwrap();
            let lower = time_y(min_start);
            let upper = if only_visual {
                lower + increase
            } else {
                let max_end = groups.iter().map(|g| g[0].end.unwrap()).max().unwrap();
                (lower + increase).max(time_y(max_end))
            };
            (lower, upper)
        }
        ItemKind::Assignment => {
            let max_end = groups.iter().filter_map(|g| g[0].end).max().unwrap();
            let upper = time_y(max_end);
            (upper - increase, upper)
        }
        ItemKind::Reminder => {
            let lower = time_y(groups[0][0].start.unwrap());
            (lower, lower + increase)
        }
        _ => (0.0, 0.0),
    }
}

// Partition by exact-time equality and merge adjacent non-event partitions
// whose visual ranges overlap. Horizontal lanes are assigned afterwards across
// all visible chunks for the day.
pub(super) fn build_chunks_for_kind<'a>(
    tasks: &[&'a CalendarTask],
    _prior: &[Vec<ScheduleChunk<'a>>],
) -> Vec<ScheduleChunk<'a>> {
    if tasks.is_empty() {
        return Vec::new();
    }

    let kind = tasks[0].kind;
    let mut sorted: Vec<&CalendarTask> = tasks.to_vec();
    if kind == ItemKind::Event {
        sorted.sort_by(|a, b| {
            let cmp = a.start.unwrap().cmp(&b.start.unwrap());
            if cmp != std::cmp::Ordering::Equal {
                cmp
            } else {
                b.end.unwrap().cmp(&a.end.unwrap())
            }
        });
    } else {
        sorted.sort_by_key(|t| t.start.or(t.end).unwrap());
    }

    // Partition by (start, end) exact equality.
    let mut partitions: Vec<Vec<&CalendarTask>> = Vec::new();
    let mut running: Vec<&CalendarTask> = vec![sorted[0]];
    for &item in sorted.iter().skip(1) {
        if running[0].start == item.start && running[0].end == item.end {
            running.push(item);
        } else {
            partitions.push(std::mem::take(&mut running));
            running.push(item);
        }
    }
    partitions.push(running);

    if kind == ItemKind::Event {
        return build_event_chunks(partitions);
    }

    let convert = |groups: Vec<Vec<&'a CalendarTask>>| -> ScheduleChunk<'a> {
        ScheduleChunk {
            equal_groups: groups,
            show_time: true,
            lane: 0,
            lane_span: 1,
            lane_count: 1,
        }
    };

    // Merge adjacent partitions whose visual ranges overlap.
    let mut chunks: Vec<ScheduleChunk<'a>> = Vec::new();
    let mut inter_running: Vec<Vec<&'a CalendarTask>> = vec![partitions.remove(0)];
    for group in partitions {
        let old_end = estimate_range_y(&inter_running, true, false).1;
        let new_group_slice = std::slice::from_ref(&group);
        let new_start = estimate_range_y(new_group_slice, true, false).0;
        if new_start >= old_end {
            let taken = std::mem::take(&mut inter_running);
            chunks.push(convert(taken));
            inter_running.push(group);
        } else {
            inter_running.push(group);
        }
    }
    chunks.push(convert(inter_running));

    chunks
}

fn build_event_chunks<'a>(partitions: Vec<Vec<&'a CalendarTask>>) -> Vec<ScheduleChunk<'a>> {
    #[derive(Debug)]
    struct PendingEventChunk<'a> {
        group: Vec<&'a CalendarTask>,
        start: chrono::DateTime<Local>,
        end: chrono::DateTime<Local>,
        lane: usize,
    }

    fn flush_component<'a>(
        component: &mut Vec<PendingEventChunk<'a>>,
        chunks: &mut Vec<ScheduleChunk<'a>>,
    ) {
        if component.is_empty() {
            return;
        }

        let lane_count = component.iter().map(|chunk| chunk.lane).max().unwrap_or(0) + 1;

        let lane_spans = component
            .iter()
            .map(|chunk| {
                let mut lane_span = 1;
                'lanes: for lane in (chunk.lane + 1)..lane_count {
                    for other in component.iter() {
                        if other.lane == lane && event_times_overlap(chunk, other) {
                            break 'lanes;
                        }
                    }
                    lane_span += 1;
                }
                lane_span
            })
            .collect::<Vec<_>>();

        for (chunk, lane_span) in component.drain(..).zip(lane_spans) {
            chunks.push(ScheduleChunk {
                equal_groups: vec![chunk.group],
                show_time: (chunk.end - chunk.start).num_minutes() > 30,
                lane: chunk.lane,
                lane_span,
                lane_count,
            });
        }
    }

    fn event_times_overlap(a: &PendingEventChunk<'_>, b: &PendingEventChunk<'_>) -> bool {
        a.start < b.end && b.start < a.end
    }

    let mut chunks = Vec::with_capacity(partitions.len());
    let mut component: Vec<PendingEventChunk<'a>> = Vec::new();
    let mut component_end: Option<chrono::DateTime<Local>> = None;
    let mut active: Vec<(chrono::DateTime<Local>, usize)> = Vec::new();

    for group in partitions {
        let start = group[0].start.unwrap();
        let end = group[0].end.unwrap();

        if component_end.is_some_and(|current_end| start >= current_end) {
            flush_component(&mut component, &mut chunks);
            active.clear();
            component_end = None;
        }

        active.retain(|(active_end, _)| *active_end > start);

        let mut lane = 0;
        while active.iter().any(|(_, active_lane)| *active_lane == lane) {
            lane += 1;
        }
        active.push((end, lane));
        component_end = Some(component_end.map_or(end, |current_end| current_end.max(end)));

        component.push(PendingEventChunk {
            group,
            start,
            end,
            lane,
        });
    }

    flush_component(&mut component, &mut chunks);

    chunks
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChunkBucket {
    Event,
    Reminder,
    Assignment,
}

#[derive(Clone, Copy, Debug)]
struct ChunkPlacement {
    bucket: ChunkBucket,
    index: usize,
    range: (f32, f32),
    lane: usize,
    lane_span: usize,
    lane_count: usize,
}

pub(super) fn assign_calendar_chunk_lanes<'a>(
    events: &mut [ScheduleChunk<'a>],
    reminders: &mut [ScheduleChunk<'a>],
    assignments: &mut [ScheduleChunk<'a>],
) {
    let mut placements = Vec::with_capacity(events.len() + reminders.len() + assignments.len());
    collect_chunk_placements(ChunkBucket::Event, events, &mut placements);
    collect_chunk_placements(ChunkBucket::Reminder, reminders, &mut placements);
    collect_chunk_placements(ChunkBucket::Assignment, assignments, &mut placements);
    if placements.is_empty() {
        return;
    }

    placements.sort_by(|a, b| {
        a.range
            .0
            .total_cmp(&b.range.0)
            .then_with(|| b.range.1.total_cmp(&a.range.1))
            .then_with(|| chunk_bucket_order(a.bucket).cmp(&chunk_bucket_order(b.bucket)))
            .then_with(|| a.index.cmp(&b.index))
    });

    let mut active: Vec<(f32, usize)> = Vec::new();
    let mut component: Vec<usize> = Vec::new();
    let mut component_end: Option<f32> = None;

    for idx in 0..placements.len() {
        let (start, end) = placements[idx].range;
        if component_end.is_some_and(|current_end| start >= current_end) {
            flush_chunk_lane_component(&mut placements, &component);
            component.clear();
            active.clear();
            component_end = None;
        }

        active.retain(|(active_end, _)| *active_end > start);

        let mut lane = 0;
        while active.iter().any(|(_, active_lane)| *active_lane == lane) {
            lane += 1;
        }
        placements[idx].lane = lane;
        active.push((end, lane));
        component_end = Some(component_end.map_or(end, |current_end| current_end.max(end)));
        component.push(idx);
    }

    flush_chunk_lane_component(&mut placements, &component);

    for placement in placements {
        let chunk = match placement.bucket {
            ChunkBucket::Event => &mut events[placement.index],
            ChunkBucket::Reminder => &mut reminders[placement.index],
            ChunkBucket::Assignment => &mut assignments[placement.index],
        };
        chunk.lane = placement.lane;
        chunk.lane_span = placement.lane_span;
        chunk.lane_count = placement.lane_count;
    }
}

fn collect_chunk_placements(
    bucket: ChunkBucket,
    chunks: &[ScheduleChunk<'_>],
    placements: &mut Vec<ChunkPlacement>,
) {
    placements.extend(chunks.iter().enumerate().map(|(index, chunk)| {
        let range = chunk_lane_range_y(chunk);
        ChunkPlacement {
            bucket,
            index,
            range,
            lane: 0,
            lane_span: 1,
            lane_count: 1,
        }
    }));
}

fn chunk_lane_range_y(chunk: &ScheduleChunk<'_>) -> (f32, f32) {
    let kind = chunk.equal_groups[0][0].kind;
    let (top, bottom) = match kind {
        ItemKind::Event => estimate_range_y(&chunk.equal_groups, chunk.show_time, false),
        _ => estimate_range_y(&chunk.equal_groups, chunk.show_time, true),
    };
    let top = top.max(TIME_Y_OFFSET);
    let min_height = if kind == ItemKind::Event { 1.0 } else { 18.0 };
    (top, bottom.max(top + min_height))
}

fn flush_chunk_lane_component(placements: &mut [ChunkPlacement], component: &[usize]) {
    if component.is_empty() {
        return;
    }

    let lane_count = component
        .iter()
        .map(|idx| placements[*idx].lane)
        .max()
        .unwrap_or(0)
        + 1;

    let lane_spans = component
        .iter()
        .map(|idx| {
            let chunk = placements[*idx];
            let mut lane_span = 1;
            'lanes: for lane in (chunk.lane + 1)..lane_count {
                for other_idx in component {
                    let other = placements[*other_idx];
                    if other.lane == lane && ranges_overlap(chunk.range, other.range) {
                        break 'lanes;
                    }
                }
                lane_span += 1;
            }
            lane_span
        })
        .collect::<Vec<_>>();

    for (idx, lane_span) in component.iter().zip(lane_spans) {
        placements[*idx].lane_span = lane_span;
        placements[*idx].lane_count = lane_count;
    }
}

fn chunk_bucket_order(bucket: ChunkBucket) -> usize {
    match bucket {
        ChunkBucket::Event => 0,
        ChunkBucket::Reminder => 1,
        ChunkBucket::Assignment => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_task(
        start_hour: u32,
        start_minute: u32,
        end_hour: u32,
        end_minute: u32,
        title: &str,
    ) -> CalendarTask {
        let start = Local
            .with_ymd_and_hms(2026, 5, 18, start_hour, start_minute, 0)
            .single()
            .unwrap();
        let end = Local
            .with_ymd_and_hms(2026, 5, 18, end_hour, end_minute, 0)
            .single()
            .unwrap();
        CalendarTask {
            scheme_id: SchemeId::new(),
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
            color_index: 0,
            is_daily: false,
            is_read_only: false,
            text: title.to_string(),
            start: Some(start),
            end: Some(end),
            kind: ItemKind::Event,
            is_done: false,
            occurrence_index: 0,
        }
    }

    fn reminder_task(hour: u32, minute: u32, title: &str) -> CalendarTask {
        let start = Local
            .with_ymd_and_hms(2026, 5, 18, hour, minute, 0)
            .single()
            .unwrap();
        CalendarTask {
            scheme_id: SchemeId::new(),
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
            color_index: 0,
            is_daily: false,
            is_read_only: false,
            text: title.to_string(),
            start: Some(start),
            end: None,
            kind: ItemKind::Reminder,
            is_done: false,
            occurrence_index: 0,
        }
    }

    fn assignment_task(hour: u32, minute: u32, title: &str) -> CalendarTask {
        let end = Local
            .with_ymd_and_hms(2026, 5, 18, hour, minute, 0)
            .single()
            .unwrap();
        CalendarTask {
            scheme_id: SchemeId::new(),
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
            color_index: 0,
            is_daily: false,
            is_read_only: false,
            text: title.to_string(),
            start: None,
            end: Some(end),
            kind: ItemKind::Assignment,
            is_done: false,
            occurrence_index: 0,
        }
    }

    #[test]
    fn exact_time_events_share_one_chunk() {
        let a = event_task(9, 0, 10, 0, "A");
        let b = event_task(9, 0, 10, 0, "B");
        let chunks = build_chunks_for_kind(&[&a, &b], &[]);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].lane, 0);
        assert_eq!(chunks[0].lane_span, 1);
        assert_eq!(chunks[0].lane_count, 1);
        assert_eq!(chunks[0].equal_groups.len(), 1);
        assert_eq!(chunks[0].equal_groups[0].len(), 2);
    }

    #[test]
    fn overlapping_events_use_nested_lane_without_merging() {
        let outer = event_task(9, 0, 10, 0, "Outer");
        let inner = event_task(9, 15, 9, 45, "Inner");
        let chunks = build_chunks_for_kind(&[&outer, &inner], &[]);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].lane, 0);
        assert_eq!(chunks[1].lane, 1);
        assert_eq!(chunks[0].lane_span, 1);
        assert_eq!(chunks[1].lane_span, 1);
        assert_eq!(chunks[0].lane_count, 2);
        assert_eq!(chunks[1].lane_count, 2);
        assert_eq!(chunks[0].equal_groups[0][0].text, "Outer");
        assert_eq!(chunks[1].equal_groups[0][0].text, "Inner");
    }

    #[test]
    fn event_reuses_full_width_when_only_nested_lane_intersects() {
        let outer = event_task(9, 0, 9, 45, "Outer");
        let inner = event_task(9, 15, 10, 45, "Inner");
        let after_outer = event_task(9, 45, 10, 30, "After outer");
        let chunks = build_chunks_for_kind(&[&outer, &inner, &after_outer], &[]);

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].lane, 0);
        assert_eq!(chunks[1].lane, 1);
        assert_eq!(chunks[2].lane, 0);
        assert_eq!(chunks[2].lane_span, 1);
        assert_eq!(chunks[2].lane_count, 2);
        assert_eq!(chunks[2].equal_groups[0][0].text, "After outer");
    }

    #[test]
    fn event_expands_into_free_lanes_inside_overlap_component() {
        let long = event_task(9, 0, 12, 0, "Long");
        let early = event_task(9, 0, 10, 0, "Early");
        let middle = event_task(9, 30, 10, 30, "Middle");
        let late = event_task(10, 30, 12, 0, "Late");
        let chunks = build_chunks_for_kind(&[&long, &early, &middle, &late], &[]);

        let late = chunks
            .iter()
            .find(|chunk| chunk.equal_groups[0][0].text == "Late")
            .unwrap();

        assert_eq!(late.lane, 1);
        assert_eq!(late.lane_span, 2);
        assert_eq!(late.lane_count, 3);
    }

    #[test]
    fn event_width_resets_after_endpoint_boundary() {
        let first = event_task(9, 0, 10, 0, "First");
        let second = event_task(9, 30, 10, 0, "Second");
        let third = event_task(10, 0, 11, 0, "Third");
        let chunks = build_chunks_for_kind(&[&first, &second, &third], &[]);

        let third = chunks
            .iter()
            .find(|chunk| chunk.equal_groups[0][0].text == "Third")
            .unwrap();

        assert_eq!(third.lane, 0);
        assert_eq!(third.lane_span, 1);
        assert_eq!(third.lane_count, 1);
    }

    #[test]
    fn shared_lanes_include_reminders_and_reset_after_overlap() {
        let event = event_task(16, 30, 17, 30, "Event");
        let reminder = reminder_task(17, 0, "Reminder");
        let later = reminder_task(18, 30, "Later");

        let mut events = build_chunks_for_kind(&[&event], &[]);
        let mut reminders = build_chunks_for_kind(&[&reminder, &later], &[]);
        let mut assignments = Vec::new();
        assign_calendar_chunk_lanes(&mut events, &mut reminders, &mut assignments);

        let reminder = reminders
            .iter()
            .find(|chunk| chunk.equal_groups[0][0].text == "Reminder")
            .unwrap();
        let later = reminders
            .iter()
            .find(|chunk| chunk.equal_groups[0][0].text == "Later")
            .unwrap();

        assert_eq!(reminder.lane_count, 2);
        assert_eq!(reminder.lane_span, 1);
        assert_eq!(later.lane, 0);
        assert_eq!(later.lane_count, 1);
        assert_eq!(later.lane_span, 1);
    }

    #[test]
    fn shared_lanes_include_assignments_and_reminders() {
        let assignment = assignment_task(17, 15, "Assignment");
        let reminder = reminder_task(17, 0, "Reminder");

        let mut events = Vec::new();
        let mut reminders = build_chunks_for_kind(&[&reminder], &[]);
        let mut assignments = build_chunks_for_kind(&[&assignment], &[]);
        assign_calendar_chunk_lanes(&mut events, &mut reminders, &mut assignments);

        assert_eq!(reminders[0].lane_count, 2);
        assert_eq!(assignments[0].lane_count, 2);
        assert_ne!(reminders[0].lane, assignments[0].lane);
    }
}
