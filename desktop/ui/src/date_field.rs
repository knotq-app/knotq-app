use std::ops::Range;
use std::time::Duration as StdDuration;

use gpui::{
    Bounds, Context, EventEmitter, FocusHandle, Pixels, SharedString, Subscription, Task, Window,
};

mod element;
mod ime;
mod input;
mod paint;
mod selection;
mod text;

pub use element::DateFieldElement;
use selection::DateFieldSelection;
pub use text::sanitize_numeric_component;

const DATE_FIELD_SELECTION_BG: u32 = 0x4f8dffff;
const DATE_FIELD_CURSOR_WIDTH: f32 = 1.5;
const DATE_FIELD_CURSOR_BLINK_DELAY: StdDuration = StdDuration::from_millis(500);
const DATE_FIELD_CURSOR_BLINK_INTERVAL: StdDuration = StdDuration::from_millis(500);

#[derive(Clone, Copy, Debug)]
pub enum DateComponentEvent {
    Change,
    Filled,
    PressEnter,
    TabForward,
    TabBackward,
    Cancel,
    Undo,
    Redo,
    Focus,
    Blur,
}

pub struct DateComponentField {
    value: String,
    placeholder: SharedString,
    max_len: usize,
    selection: DateFieldSelection,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    select_all_on_focus: bool,
    cursor_blink_state: bool,
    cursor_blink_task: Option<Task<()>>,
    pub focus_handle: FocusHandle,
    last_bounds: Option<Bounds<Pixels>>,
    _focus_in_subscription: Subscription,
    _focus_out_subscription: Subscription,
}

impl EventEmitter<DateComponentEvent> for DateComponentField {}

impl DateComponentField {
    pub fn new(
        placeholder: impl Into<SharedString>,
        value: impl Into<String>,
        max_len: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle().tab_stop(true);
        let focus_in_subscription = cx.on_focus_in(&focus_handle, window, |field, _window, cx| {
            if field.select_all_on_focus {
                field.select_all(cx);
            } else {
                field.select_all_on_focus = true;
            }
            field.reset_cursor_blink(cx);
            cx.emit(DateComponentEvent::Focus);
        });
        let focus_out_subscription =
            cx.on_focus_out(&focus_handle, window, |field, _event, _window, cx| {
                field.is_selecting = false;
                field.select_all_on_focus = true;
                field.cursor_blink_task = None;
                field.cursor_blink_state = false;
                cx.emit(DateComponentEvent::Blur);
                cx.notify();
            });

        let mut field = Self {
            value: String::new(),
            placeholder: placeholder.into(),
            max_len,
            selection: DateFieldSelection::collapsed(0),
            marked_range: None,
            is_selecting: false,
            select_all_on_focus: true,
            cursor_blink_state: true,
            cursor_blink_task: None,
            focus_handle,
            last_bounds: None,
            _focus_in_subscription: focus_in_subscription,
            _focus_out_subscription: focus_out_subscription,
        };
        field.set_value(value.into(), cx);
        field.selection = DateFieldSelection {
            anchor: 0,
            head: field.value.len(),
        };
        field
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        let value = sanitize_numeric_component(&value.into(), self.max_len);
        if self.value != value {
            self.value = value;
        }
        let cursor = self.value.len();
        self.selection = DateFieldSelection::collapsed(cursor);
        self.marked_range = None;
        cx.notify();
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_all_on_focus = true;
        self.select_all(cx);
        self.reset_cursor_blink(cx);
        self.focus_handle.focus(window);
        cx.notify();
    }
}
