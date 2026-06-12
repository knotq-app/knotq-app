use chrono::{Datelike, Duration, Local, TimeZone, Timelike, Utc, Weekday};
use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, CursorStyle, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, SharedString,
};
use gpui_component::scroll::ScrollableElement as _;
use knotq_model::{ItemId, ItemKind, OccurrenceId, SchemeId};
use knotq_rrule::ItemOccurrenceExt;
use knotq_storage_json::CalendarViewMode;

use crate::app::{
    daily_queue_marker_color, CalendarDragState, CalendarMoveState, CalendarResizeState, KnotQApp,
    CALENDAR_WEEK_VIEW_DAYS,
};
use crate::theme_gpui::{
    calendar_item_color, date_status_color, event_status_color, token_hsla, token_rgba, Theme,
    FONT_DISPLAY, FONT_MONO, FONT_SIZE_BODY, FONT_SIZE_CALENDAR_ITEM, FONT_SIZE_CALENDAR_TIME,
    FONT_SIZE_CAPTION2, FONT_UI,
};
use knotq_date_util::{format_hour_label, format_time};
use knotq_storage_json::TimeFormat;

pub(super) const HOUR_H: f32 = 40.0;
pub(super) const TIME_Y_OFFSET: f32 = 12.0;

// Overlap/layout heuristics (ported from knotqv1 CalendarView.swift).
// Hours are converted to pixels via HOUR_H.
pub(super) const OVERLAP_OFFSET: f32 = 15.0;
pub(super) const BASE_HEIGHT_HOURS: f32 = 0.125;
pub(super) const RUN_LINE_HOURS: f32 = 0.4;
pub(super) const TIME_HEADER_HOURS: f32 = 0.3;
pub(super) const MIN_WEEK_DAY_W: f32 = 100.0;
pub(super) const MONTH_WEEKDAYS: [Weekday; 7] = [
    Weekday::Sun,
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
];

#[derive(Clone, Debug)]
pub(super) struct CalendarTask {
    scheme_id: SchemeId,
    item_id: ItemId,
    occurrence: OccurrenceId,
    color_index: u8,
    is_daily: bool,
    is_read_only: bool,
    text: String,
    start: Option<chrono::DateTime<Local>>,
    end: Option<chrono::DateTime<Local>>,
    kind: ItemKind,
    is_done: bool,
    occurrence_index: usize,
}

#[derive(Clone, Debug)]
pub(super) struct CalendarPopupTarget {
    scheme_id: SchemeId,
    item_id: ItemId,
    occurrence: OccurrenceId,
    occurrence_index: usize,
    start: Option<chrono::DateTime<Utc>>,
    end: Option<chrono::DateTime<Utc>>,
    is_read_only: bool,
}

impl CalendarPopupTarget {
    pub(super) fn from_task(task: &CalendarTask) -> Self {
        Self {
            scheme_id: task.scheme_id,
            item_id: task.item_id,
            occurrence: task.occurrence.clone(),
            occurrence_index: task.occurrence_index,
            start: task.start.as_ref().map(|dt| dt.with_timezone(&Utc)),
            end: task.end.as_ref().map(|dt| dt.with_timezone(&Utc)),
            is_read_only: task.is_read_only,
        }
    }
}

#[derive(Clone)]
pub(super) struct ScheduleChunk<'a> {
    equal_groups: Vec<Vec<&'a CalendarTask>>,
    show_time: bool,
    offset: f32,
    lane: usize,
}

mod layout;
mod month;
mod render_blocks;
mod tasks;
mod week;

use self::layout::*;
use self::render_blocks::*;

impl KnotQApp {
    pub fn render_calendar(
        &mut self,
        available_width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match self.calendar_view {
            CalendarViewMode::Week => self.render_week_calendar(available_width, cx),
            CalendarViewMode::Month => self.render_month_calendar(available_width, cx),
        }
    }
}
