use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};

use knotq_model::{Item, ItemKind, Occurrence, OccurrenceId, RepeatWeekday, SimpleRecurrence};

use crate::expand::{
    materialize_occurrence, occurrence_hits_range, occurrence_sort_key, ExpansionCtx,
};
use crate::ical::repeat_weekday_from_chrono;
use crate::repeat_end::repeat_end_allows;

pub(crate) fn expand_simple(
    item: &Item,
    kind: ItemKind,
    anchor: DateTime<Utc>,
    recurrence: &SimpleRecurrence,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<Occurrence> {
    let ctx = ExpansionCtx {
        item,
        kind,
        anchor,
        from,
        to,
    };
    match recurrence {
        SimpleRecurrence::Daily { .. } => {
            expand_linear_days(ctx, recurrence, recurrence.interval())
        }
        SimpleRecurrence::Weekly { weekdays, .. } => {
            expand_weekly(ctx, recurrence, recurrence.interval(), weekdays)
        }
        SimpleRecurrence::Monthly { .. } => {
            expand_months_or_years(ctx, recurrence, recurrence.interval(), false)
        }
        SimpleRecurrence::Yearly { .. } => {
            expand_months_or_years(ctx, recurrence, recurrence.interval(), true)
        }
    }
}

pub(crate) fn expand_linear_days(
    ctx: ExpansionCtx<'_>,
    recurrence: &SimpleRecurrence,
    interval_days: usize,
) -> Vec<Occurrence> {
    let mut out = Vec::new();
    let interval_days = interval_days.max(1);
    let interval = Duration::days(interval_days as i64);
    let mut index = first_linear_day_index(ctx, interval_days);
    let mut current = ctx.anchor + interval * index as i32;
    while current < ctx.to {
        if !repeat_end_allows(recurrence.repeat_end(), index, current) {
            break;
        }
        maybe_push_occurrence(&mut out, ctx, current, index);
        index += 1;
        current = ctx.anchor + interval * index as i32;
    }
    out
}

pub(crate) fn expand_weekly(
    ctx: ExpansionCtx<'_>,
    recurrence: &SimpleRecurrence,
    interval_weeks: usize,
    weekdays: &[RepeatWeekday],
) -> Vec<Occurrence> {
    let mut selected = if weekdays.is_empty() {
        vec![repeat_weekday_from_chrono(ctx.anchor.weekday())]
    } else {
        weekdays.to_vec()
    };
    selected.sort_unstable();
    selected.dedup();

    let anchor_week_start = ctx.anchor.date_naive()
        - Duration::days(ctx.anchor.weekday().num_days_from_monday() as i64);
    let mut out = Vec::new();
    let mut cycle = first_weekly_cycle(ctx, anchor_week_start);
    let interval = interval_weeks.max(1);
    let mut generated =
        weekly_generated_before_cycle(cycle, interval, &selected, anchor_week_start, ctx.anchor);

    loop {
        let week_start = anchor_week_start + Duration::weeks(cycle as i64);
        if week_start > ctx.to.date_naive() + Duration::weeks(1) {
            break;
        }

        if cycle.is_multiple_of(interval) {
            for weekday in &selected {
                let day = week_start + Duration::days(weekday.num_days_from_monday() as i64);
                let Some(current) = Utc
                    .with_ymd_and_hms(
                        day.year(),
                        day.month(),
                        day.day(),
                        ctx.anchor.hour(),
                        ctx.anchor.minute(),
                        ctx.anchor.second(),
                    )
                    .single()
                else {
                    continue;
                };
                if current < ctx.anchor {
                    continue;
                }
                if !repeat_end_allows(recurrence.repeat_end(), generated, current) {
                    return out;
                }
                maybe_push_occurrence(&mut out, ctx, current, generated);
                generated += 1;
            }
        }
        cycle += 1;
    }
    out
}

pub(crate) fn expand_months_or_years(
    ctx: ExpansionCtx<'_>,
    recurrence: &SimpleRecurrence,
    interval: usize,
    yearly: bool,
) -> Vec<Occurrence> {
    let mut out = Vec::new();
    let month_stride = if yearly { 12 } else { 1 } * interval.max(1);
    let mut step = first_month_step(ctx, month_stride);
    let Some(mut generated) = generated_before_month_step(ctx, recurrence, step, month_stride)
    else {
        return out;
    };
    let mut invalid_steps = 0usize;

    loop {
        let Some(current) = add_months_exact(ctx.anchor, step * month_stride) else {
            step += 1;
            invalid_steps += 1;
            if invalid_steps > 4096 {
                break;
            }
            continue;
        };
        invalid_steps = 0;
        if current >= ctx.to {
            break;
        }
        if !repeat_end_allows(recurrence.repeat_end(), generated, current) {
            break;
        }
        maybe_push_occurrence(&mut out, ctx, current, generated);
        generated += 1;
        step += 1;
    }
    out
}

fn effective_search_start(ctx: ExpansionCtx<'_>) -> DateTime<Utc> {
    let max_positive_offset = [ctx.item.start, ctx.item.end, ctx.item.available]
        .into_iter()
        .flatten()
        .map(|dt| dt - ctx.anchor)
        .filter(|offset| *offset > Duration::zero())
        .max()
        .unwrap_or_else(Duration::zero);
    ctx.from - max_positive_offset
}

fn first_linear_day_index(ctx: ExpansionCtx<'_>, interval_days: usize) -> usize {
    let target = effective_search_start(ctx);
    if target <= ctx.anchor {
        return 0;
    }
    let interval_secs = interval_days.max(1) as i64 * 24 * 60 * 60;
    let delta_secs = target.signed_duration_since(ctx.anchor).num_seconds();
    ((delta_secs + interval_secs - 1) / interval_secs) as usize
}

fn first_weekly_cycle(ctx: ExpansionCtx<'_>, anchor_week_start: chrono::NaiveDate) -> usize {
    let target = effective_search_start(ctx).date_naive();
    if target <= anchor_week_start {
        return 0;
    }
    (target - anchor_week_start).num_days().div_euclid(7) as usize
}

fn weekly_generated_before_cycle(
    cycle: usize,
    interval: usize,
    selected: &[RepeatWeekday],
    anchor_week_start: chrono::NaiveDate,
    anchor: DateTime<Utc>,
) -> usize {
    if cycle == 0 || selected.is_empty() {
        return 0;
    }
    let active_cycles = ((cycle - 1) / interval.max(1)) + 1;
    let mut generated = active_cycles * selected.len();

    for weekday in selected {
        let day = anchor_week_start + Duration::days(weekday.num_days_from_monday() as i64);
        let current = Utc
            .with_ymd_and_hms(
                day.year(),
                day.month(),
                day.day(),
                anchor.hour(),
                anchor.minute(),
                anchor.second(),
            )
            .single()
            .unwrap_or(anchor);
        if current < anchor {
            generated = generated.saturating_sub(1);
        }
    }

    generated
}

fn first_month_step(ctx: ExpansionCtx<'_>, month_stride: usize) -> usize {
    let target = effective_search_start(ctx);
    let anchor_month = ctx.anchor.year() as i64 * 12 + ctx.anchor.month0() as i64;
    let target_month = target.year() as i64 * 12 + target.month0() as i64;
    if target_month <= anchor_month {
        return 0;
    }
    ((target_month - anchor_month) as usize) / month_stride.max(1)
}

fn generated_before_month_step(
    ctx: ExpansionCtx<'_>,
    recurrence: &SimpleRecurrence,
    step: usize,
    month_stride: usize,
) -> Option<usize> {
    let mut generated = 0usize;
    for previous_step in 0..step {
        let Some(current) = add_months_exact(ctx.anchor, previous_step * month_stride) else {
            continue;
        };
        if !repeat_end_allows(recurrence.repeat_end(), generated, current) {
            return None;
        }
        generated += 1;
    }
    Some(generated)
}

fn maybe_push_occurrence(
    out: &mut Vec<Occurrence>,
    ctx: ExpansionCtx<'_>,
    current_anchor: DateTime<Utc>,
    occurrence_index: usize,
) {
    let occurrence = materialize_occurrence(
        ctx.item,
        ctx.kind,
        OccurrenceId::recurring_utc(current_anchor),
        occurrence_index,
        ctx.anchor,
        current_anchor,
    );
    let anchor = occurrence_sort_key(&occurrence).unwrap_or(current_anchor);
    if occurrence_hits_range(occurrence.start, occurrence.end, anchor, ctx.from, ctx.to) {
        out.push(occurrence);
    }
}

fn add_months_exact(anchor: DateTime<Utc>, months: usize) -> Option<DateTime<Utc>> {
    let month0 = anchor.month0() as usize;
    let total = anchor.year() as i64 * 12 + month0 as i64 + months as i64;
    let year = total.div_euclid(12) as i32;
    let month = total.rem_euclid(12) as u32 + 1;
    Utc.with_ymd_and_hms(
        year,
        month,
        anchor.day(),
        anchor.hour(),
        anchor.minute(),
        anchor.second(),
    )
    .single()
}
