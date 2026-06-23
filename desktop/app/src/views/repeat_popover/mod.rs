use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use gpui::prelude::*;
use gpui::{deferred, div, px, ClickEvent, Context, FontWeight, IntoElement, MouseButton, Window};
use gpui_component::input::Escape as InputEscape;
use knotq_commands::Command;
use knotq_model::{
    CalendarRecurrence, Item, ItemId, Recurrence, RepeatEnd, RepeatWeekday, SchemeId,
    SimpleRecurrence,
};

use crate::app::{KnotQApp, RepeatPopover, RepeatScope};
use crate::theme_gpui::{selected_date_text_color, token_hsla, token_rgba, Theme};
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

pub(crate) const REPEAT_POPOVER_WIDTH: f32 = 286.0;
const REPEAT_POPOVER_PRIORITY: usize = 20_500;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RepeatMode {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl RepeatMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Yearly => "Yearly",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RepeatState {
    mode: RepeatMode,
    interval: usize,
    weekdays: Vec<RepeatWeekday>,
    end: RepeatEnd,
}

impl Default for RepeatState {
    fn default() -> Self {
        Self {
            mode: RepeatMode::Daily,
            interval: 1,
            weekdays: Vec::new(),
            end: RepeatEnd::Never,
        }
    }
}

impl RepeatState {
    fn normalized(mut self, item: &Item) -> Self {
        self.interval = self.interval.max(1);
        if self.mode == RepeatMode::Weekly {
            if self.weekdays.is_empty() {
                self.weekdays = vec![default_weekday_for_item(item)];
            }
            self.weekdays
                .sort_unstable_by_key(|day| weekday_index(*day) as usize);
            self.weekdays.dedup();
        } else {
            self.weekdays.clear();
        }
        if let RepeatEnd::Count(count) = &mut self.end {
            *count = (*count).max(1);
        }
        self
    }
}

#[derive(Clone, Copy)]
pub(super) struct RepeatTarget {
    scheme_id: SchemeId,
    item_id: ItemId,
}

mod actions;
mod components;
mod recurrence;
mod render;
mod until_calendar;

use crate::views::{repeat_end_for_local_date, UNTIL_CALENDAR_HEIGHT, UNTIL_CALENDAR_WIDTH};
use self::components::*;
use self::recurrence::*;
use self::until_calendar::*;
