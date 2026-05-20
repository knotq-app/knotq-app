use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use gpui::prelude::*;
use gpui::{
    deferred, div, px, ClickEvent, Context, CursorStyle, Entity, IntoElement, MouseButton, Window,
};
use gpui_component::input::Escape as InputEscape;
use knotq_commands::{Command, DateKind};
use knotq_model::{Item, ItemId, SchemeId};
use knotq_storage_json::TimeFormat;

use crate::app::{DatePickerPopover, KnotQApp};
use crate::theme_gpui::{
    token_hsla, token_rgba, Theme, FONT_MONO, FONT_SIZE_BODY, FONT_SIZE_CAPTION2, FONT_UI,
};
use knotq_ui::date_field::{DateComponentEvent, DateComponentField, DateFieldElement};
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

pub(super) const DATE_FIELD_HEIGHT: f32 = 22.0;
pub(super) const DATE_FIELD_TEXT_SIZE: f32 = FONT_SIZE_BODY;
pub(super) const DATE_POPOVER_PRIORITY: usize = 20_000;
pub(crate) const DATE_POPOVER_WIDTH_24H: f32 = 278.0;
pub(crate) const DATE_POPOVER_WIDTH_12H: f32 = 304.0;
pub(super) const DATE_POPOVER_HEIGHT: f32 = 232.0;
pub(super) const DATE_GROUP_PAD_X: f32 = 3.0;

#[derive(Clone, Copy)]
pub(super) struct DateTarget {
    scheme_id: knotq_model::SchemeId,
    item_id: knotq_model::ItemId,
    kind: DateKind,
}

mod actions;
mod fields;
mod render;
mod time;

use self::fields::*;
use self::time::*;
