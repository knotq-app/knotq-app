use chrono::{Duration, Local, TimeZone, Utc};
use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, Hsla, IntoElement, MouseButton, MouseDownEvent};
use gpui_component::scroll::ScrollableElement as _;
use knotq_model::{ItemId, ItemKind, OccurrenceId, SchemeId};
use knotq_rrule::ItemOccurrenceExt;

use crate::app::{daily_queue_marker_color, KnotQApp};
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

mod formatting;
mod render;

use self::formatting::*;
