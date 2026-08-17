use chrono::{Datelike, Duration, Local, TimeZone, Utc};
use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, Hsla, IntoElement, MouseButton, MouseDownEvent};
use gpui_component::scroll::ScrollableElement as _;
use knotq_model::{Item, ItemId, ItemKind, Occurrence, OccurrenceId, SchemeId};
use knotq_rrule::ItemOccurrenceExt;

use crate::app::{daily_queue_marker_color, KnotQApp, OpenEventPopupArgs};
use crate::theme_gpui::{
    date_status_color, event_status_color, token_hsla, token_rgba, upcoming_scheme_color,
    FONT_MONO, FONT_SIZE_BODY, FONT_SIZE_CAPTION2,
};
use knotq_date_util::{format_time, upcoming_range};

#[derive(Clone)]
pub(super) struct UpRow {
    scheme_id: SchemeId,
    item_id: ItemId,
    occurrence: OccurrenceId,
    occurrence_index: usize,
    scheme_name: String,
    color_index: u8,
    is_daily: bool,
    text: String,
    is_done: bool,
    when_label: String,
    date_color: Hsla,
    sort_key: chrono::DateTime<chrono::Utc>,
    start: Option<chrono::DateTime<chrono::Utc>>,
    end: Option<chrono::DateTime<chrono::Utc>>,
}

/// The rows the panel shows, already filtered, sorted and truncated.
///
/// Producing this scans every item of every scheme and expands each one's
/// recurrence, so it is cached against [`UpcomingRowsKey`] rather than redone on
/// every render — the root view re-renders on every keystroke, and typing into
/// an item's text cannot change what is upcoming.
#[derive(Clone, Default)]
pub(crate) struct UpcomingRows {
    assignments: Vec<UpRow>,
    reminders: Vec<UpRow>,
    upcoming: Vec<UpRow>,
}

impl UpcomingRows {
    fn iter_mut(&mut self) -> impl Iterator<Item = &mut UpRow> {
        self.assignments
            .iter_mut()
            .chain(self.reminders.iter_mut())
            .chain(self.upcoming.iter_mut())
    }
}

/// Everything [`UpcomingRows`] is derived from. `second` is the wall clock
/// truncated to a second: the rows depend on "now" (overdue styling, relative
/// labels, the horizon cut-off), so the cache must expire with it, but no
/// user-visible detail changes within one second. The theme enters as the pair
/// that selects it rather than as a colour, so two themes that happen to share a
/// light/dark polarity cannot collide.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct UpcomingRowsKey {
    schedule_revision: u64,
    second: i64,
    time_format: knotq_model::TimeFormat,
    theme_mode: knotq_model::ThemeMode,
    system_theme_dark: bool,
}

pub(crate) struct UpcomingCache {
    key: UpcomingRowsKey,
    rows: UpcomingRows,
}

mod formatting;
mod render;

use self::formatting::*;
