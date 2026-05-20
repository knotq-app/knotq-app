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
    let mut index = 0usize;
    let interval = Duration::days(interval_days.max(1) as i64);
    let mut current = ctx.anchor;
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
    let mut generated = 0usize;
    let mut cycle = 0usize;
    let interval = interval_weeks.max(1);

    loop {
        let week_start = anchor_week_start + Duration::weeks(cycle as i64);
        let week_end = week_start + Duration::weeks(1);
        let week_end_dt = Utc
            .with_ymd_and_hms(
                week_end.year(),
                week_end.month(),
                week_end.day(),
                ctx.anchor.hour(),
                ctx.anchor.minute(),
                ctx.anchor.second(),
            )
            .single()
            .unwrap_or(ctx.anchor);
        if week_end_dt >= ctx.to && !out.is_empty() {
            break;
        }
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
    let mut generated = 0usize;
    let mut step = 0usize;
    let month_stride = if yearly { 12 } else { 1 } * interval.max(1);

    loop {
        let Some(current) = add_months_exact(ctx.anchor, step * month_stride) else {
            step += 1;
            if step > 4096 {
                break;
            }
            continue;
        };
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
