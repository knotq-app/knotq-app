use std::collections::HashMap;

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
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
use knotq_date_util::{format_time, upcoming_range, UPCOMING_HORIZON_DAYS};

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
#[derive(Clone, Default)]
pub(crate) struct UpcomingRows {
    assignments: Vec<UpRow>,
    reminders: Vec<UpRow>,
    upcoming: Vec<UpRow>,
}

/// Which of the three passes over an item produced a candidate. Each keeps its
/// own clock-dependent filters, so the pass has to survive into phase 2.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CandidateSource {
    /// An occurrence inside the two-week horizon.
    Window,
    /// A missed instance of a repeating item, from before today.
    Overdue,
    /// A non-repeating item whose date has already passed.
    PastSingle,
}

/// One occurrence that could reach the panel, with every field that does *not*
/// depend on the wall clock already resolved.
///
/// Producing these expands recurrence and is the expensive half of the panel;
/// see the module docs on [`scan`].
#[derive(Clone)]
pub(super) struct Candidate {
    item_id: ItemId,
    /// Where the item sat in its scheme when phase 1 ran, as a hint only — the
    /// id is still checked, and a miss falls back to a search. See
    /// [`scan::candidate_text`].
    item_index: usize,
    occurrence: OccurrenceId,
    occurrence_index: usize,
    kind: ItemKind,
    start: Option<chrono::DateTime<chrono::Utc>>,
    end: Option<chrono::DateTime<chrono::Utc>>,
    available: Option<chrono::DateTime<chrono::Utc>>,
    /// The instant at which this occurrence starts overlapping the query window,
    /// i.e. the value the horizon is compared against. Lets phase 2 re-apply the
    /// *exact* horizon after phase 1 expanded a slightly wider, day-aligned one.
    enters_window_at: chrono::DateTime<chrono::Utc>,
    /// When the row sorts and when it counts as overdue.
    trigger: chrono::DateTime<chrono::Utc>,
    is_done: bool,
    repeats: bool,
    source: CandidateSource,
}

/// One scheme's candidates, and the [`AppState::scheme_schedule_revision`] they
/// were built from.
pub(super) struct SchemeCandidates {
    revision: u64,
    candidates: Vec<Candidate>,
}

/// The panel's cached phase-1 output.
///
/// Keyed by the local date rather than by the current second: the candidate set
/// is derived from a query window that starts at midnight and ends a fixed
/// number of whole days later, so it survives every tick of the clock within a
/// day. Per scheme inside that, so a schedule edit rebuilds one scheme.
pub(crate) struct UpcomingCache {
    day: NaiveDate,
    schemes: HashMap<SchemeId, SchemeCandidates>,
}

impl UpcomingCache {
    fn new(day: NaiveDate) -> Self {
        Self {
            day,
            schemes: HashMap::new(),
        }
    }
}

/// The instants phase 2 filters and formats against.
#[derive(Clone, Copy)]
pub(super) struct RowClock {
    now: chrono::DateTime<chrono::Utc>,
    /// Midnight tomorrow: an event triggering after it is not "today".
    today_end: chrono::DateTime<chrono::Utc>,
    horizon: chrono::DateTime<chrono::Utc>,
}

/// One scheme's input to phase 2: its cached candidates plus the live values the
/// rows display. Nothing here is cached — a scheme's name, colour and
/// daily-queue-ness are cheap to read and, unlike the candidates, are not all
/// covered by the per-scheme revision.
pub(super) struct SchemeRowSource<'a> {
    scheme_id: SchemeId,
    display_name: &'a str,
    color_index: u8,
    is_daily: bool,
    items: &'a [Item],
    candidates: &'a [Candidate],
}

mod formatting;
mod render;
mod scan;
#[cfg(test)]
mod tests;

use self::formatting::*;
