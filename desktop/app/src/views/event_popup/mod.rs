use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use gpui::prelude::*;
use gpui::{
    deferred, div, point, px, ClickEvent, Context, Entity, FontWeight, IntoElement, Pixels,
    SharedString, Window,
};
use gpui_component::{Icon, Sizable};
use knotq_commands::{Command, DateKind};
use knotq_model::{
    CalendarRecurrence, FolderId, Item, ItemId, ItemKind, NodeRef, OccurrenceId, Recurrence,
    RepeatEnd, RepeatWeekday, SchemeId, SimpleRecurrence,
};
use knotq_storage_json::{NotificationDefaults, TimeFormat};

use crate::app::{
    EventScopeAction, KnotQApp, RepeatScope, daily_queue_marker_color, DAILY_QUEUE_TITLE,
};
use crate::theme_gpui::{calendar_item_color, token_hsla, token_rgba, Theme, FONT_UI};
use crate::views::date_popover::{DATE_POPOVER_WIDTH_12H, DATE_POPOVER_WIDTH_24H};
use knotq_date_util::{format_date_time, format_time};
use knotq_ui::checkbox::task_checkbox;
use knotq_ui::single_line_editor::SingleLineEditor;
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

const EVENT_POPUP_PRIORITY: usize = 10_000;
const UNTIL_CALENDAR_WIDTH: f32 = 220.0;
const UNTIL_CALENDAR_HEIGHT: f32 = 211.0;
const EVENT_POPUP_WIDTH: f32 = 286.0;
const DATE_POPOVER_Y_OFFSET: f32 = 8.0;
const NOTIFICATION_MENU_WIDTH: f32 = 176.0;
const NOTIFICATION_MENU_HEIGHT: f32 = 150.0;
const NOTIFICATION_MENU_LEFT_OFFSET: f32 = 118.0;
const REPEAT_MENU_WIDTH: f32 = EVENT_POPUP_WIDTH;
const REPEAT_MENU_HEIGHT: f32 = 130.0;
const REPEAT_MENU_LEFT_OFFSET: f32 = 0.0;
const REPEAT_MENU_TOP_OFFSET: f32 = 126.0;
const SCHEME_PICKER_WIDTH: f32 = 230.0;
const SCHEME_PICKER_FOLDER_ICON: &str = "icons/zed-folder.svg";
const SCHEME_PICKER_FOLDER_ICON_SIZE: f32 = 10.5;
const SCHEME_PICKER_MOVE_ICON: &str = "icons/move-to-folder.svg";
const EVENT_POPUP_ESTIMATED_HEIGHT: f32 = 220.0;
const SCOPE_DIALOG_WIDTH: f32 = 340.0;
const EVENT_POPUP_DETAIL_LABEL_W: f32 = 112.0;
const EVENT_POPUP_DETAIL_GAP: f32 = 8.0;
const EVENT_POPUP_HEADER_GAP: f32 = 6.0;
const EVENT_POPUP_HEADER_LIP_H: f32 = 3.0;
const EVENT_POPUP_HEADER_SEPARATOR_H: f32 = 2.0;

mod actions;
mod components;
mod formatting;
mod layer;
mod recurrence;
#[cfg(test)]
mod recurrence_tests;
mod render;
mod repeat_menu;
mod repeat_weekdays;
mod scope_dialog;
mod until_calendar;

use self::components::*;
use self::formatting::*;
use self::layer::*;
use self::recurrence::*;
use self::repeat_menu::*;
use self::repeat_weekdays::*;
use self::scope_dialog::*;
use self::until_calendar::*;
