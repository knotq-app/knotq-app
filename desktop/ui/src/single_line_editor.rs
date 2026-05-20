use std::ops::Range;
use std::time::Duration as StdDuration;

use gpui::prelude::*;
use gpui::{
    div, px, App, Bounds, Context, CursorStyle, EventEmitter, FocusHandle, IntoElement,
    MouseButton, Pixels, Render, Rgba, ShapedLine, SharedString, Subscription, Task, Window,
};
use gpui_component::ActiveTheme;

mod element;
mod ime;
mod input;
mod selection;
mod text;

use element::SingleLineTextElement;
use selection::TextSelection;
use text::sanitize_input;

const CURSOR_WIDTH: Pixels = px(1.5);
const CURSOR_BLINK_DELAY: StdDuration = StdDuration::from_millis(500);
const CURSOR_BLINK_INTERVAL: StdDuration = StdDuration::from_millis(500);

fn inline_selection_rgba(cx: &App) -> Rgba {
    if cx.theme().is_dark() {
        Rgba {
            r: 0.31,
            g: 0.55,
            b: 1.0,
            a: 0.53,
        }
    } else {
        Rgba {
            r: 0.12,
            g: 0.37,
            b: 1.0,
            a: 0.36,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SingleLineEditorEvent {
    Change,
    Submit,
    Cancel,
    Focus,
    Blur,
}

pub struct SingleLineEditor {
    value: String,
    placeholder: SharedString,
    selection: TextSelection,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    focus_handle: FocusHandle,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    cursor_visible: bool,
    cursor_blink_task: Option<Task<()>>,
    _focus_in_subscription: Subscription,
    _focus_out_subscription: Subscription,
}

impl EventEmitter<SingleLineEditorEvent> for SingleLineEditor {}

impl SingleLineEditor {
    pub fn new(
        placeholder: impl Into<SharedString>,
        value: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle().tab_stop(true);
        let focus_in_subscription = cx.on_focus_in(&focus_handle, window, |editor, _window, cx| {
            editor.reset_cursor_blink(cx);
            cx.emit(SingleLineEditorEvent::Focus);
        });
        let focus_out_subscription =
            cx.on_focus_out(&focus_handle, window, |editor, _event, _window, cx| {
                editor.is_selecting = false;
                editor.marked_range = None;
                editor.cursor_visible = false;
                editor.cursor_blink_task = None;
                cx.emit(SingleLineEditorEvent::Blur);
                cx.notify();
            });

        let value = sanitize_input(value.into());
        let cursor = value.len();
        Self {
            value,
            placeholder: placeholder.into(),
            selection: TextSelection::collapsed(cursor),
            marked_range: None,
            is_selecting: false,
            focus_handle,
            last_layout: None,
            last_bounds: None,
            cursor_visible: true,
            cursor_blink_task: None,
            _focus_in_subscription: focus_in_subscription,
            _focus_out_subscription: focus_out_subscription,
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window);
        self.select_all(cx);
        self.reset_cursor_blink(cx);
    }
}

impl gpui::Focusable for SingleLineEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SingleLineEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("single-line-editor")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .flex()
            .items_center()
            .w_full()
            .h_full()
            .overflow_hidden()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(SingleLineTextElement {
                editor: cx.entity(),
            })
    }
}
