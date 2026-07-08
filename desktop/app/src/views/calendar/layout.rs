use super::*;
use std::collections::HashSet;

pub(super) fn calendar_item_title(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        knotq_l10n::t("calendar.task.untitled").to_string()
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

// Port of CalendarEvents.init in knotqv1: partition by exact-time equality,
// merge adjacent partitions whose visual ranges overlap, then assign
// horizontal offset/show_time by checking against prior types' chunks.
pub(super) fn build_chunks_for_kind<'a>(
    tasks: &[&'a CalendarTask],
    prior: &[Vec<ScheduleChunk<'a>>],
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
        if prior.iter().all(|p| p.is_empty()) {
            return ScheduleChunk {
                equal_groups: groups,
                show_time: true,
                offset: 0.0,
                lane: 0,
            };
        }
        let eff = estimate_range_y(&groups, true, true);
        let eff_clipped = estimate_range_y(&groups, false, true);
        let mut show_time = true;
        let mut offset: f32 = 0.0;
        for prior_group in prior {
            for chunk in prior_group {
                let r = estimate_range_y(&chunk.equal_groups, chunk.show_time, true);
                if ranges_overlap(eff, r) {
                    if ranges_overlap(eff_clipped, r) {
                        offset = offset.max(chunk.offset + OVERLAP_OFFSET);
                        show_time = false;
                    } else {
                        show_time = false;
                    }
                }
            }
        }
        ScheduleChunk {
            equal_groups: groups,
            show_time,
            offset,
            lane: 0,
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
    let mut chunks = Vec::with_capacity(partitions.len());
    let mut active: Vec<(chrono::DateTime<Local>, usize)> = Vec::new();

    for group in partitions {
        let start = group[0].start.unwrap();
        let end = group[0].end.unwrap();
        active.retain(|(active_end, _)| *active_end > start);

        let used = active.iter().map(|(_, lane)| *lane).collect::<HashSet<_>>();
        let mut lane = 0;
        while used.contains(&lane) {
            lane += 1;
        }
        active.push((end, lane));

        chunks.push(ScheduleChunk {
            equal_groups: vec![group],
            show_time: (end - start).num_minutes() > 30,
            offset: 0.0,
            lane,
        });
    }

    chunks
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

    #[test]
    fn exact_time_events_share_one_chunk() {
        let a = event_task(9, 0, 10, 0, "A");
        let b = event_task(9, 0, 10, 0, "B");
        let chunks = build_chunks_for_kind(&[&a, &b], &[]);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].lane, 0);
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
        assert_eq!(chunks[2].equal_groups[0][0].text, "After outer");
    }
}
