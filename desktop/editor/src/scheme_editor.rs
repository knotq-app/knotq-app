use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    actions, div, fill, point, px, quad, relative, size, App, BorderStyle, Bounds, ClipboardItem,
    Context, Corners, CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    EventEmitter, FocusHandle, GlobalElementId, Image, IntoElement, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point,
    ScrollHandle, SharedString, Style, Subscription, Task, TextAlign, TextRun, Window,
};
use knotq_commands::{Command, DateKind};
use knotq_model::TimeFormat;
use knotq_model::{Item, ItemId, ItemMarker, ItemMedia, OccurrenceId, Scheme, SchemeId};
use uuid::Uuid;

use crate::line_map::{LineMap, SchemeItemAnnotation, SchemeItemLine, TextLocation};
use crate::theme_gpui::{
    token_hsla, token_rgba, Theme, FONT_MONO, FONT_SIZE_CAPTION2, FONT_SIZE_EDITOR, FONT_UI,
};
mod annotations;
mod buffer;
mod clipboard;
mod clipboard_ops;
mod deletion;
mod geometry;
mod input_handler;
mod items;
mod keymap;
mod layout;
mod line_actions;
mod markdown;
mod markdown_actions;
mod markers;
mod media;
mod mouse;
mod movement;
mod navigation;
mod painting;
mod render;
mod scrolling;
mod selection;
mod state;
mod text_edit;
mod utf16;

pub use keymap::init;

use annotations::*;
use buffer::*;
use clipboard::*;
use geometry::bounds_contains;
use items::*;
use markdown::*;
use media::*;
use navigation::*;
use selection::TextSelection;

actions!(
    scheme_editor,
    [
        Backspace,
        BackspaceLine,
        BackspaceWord,
        Copy,
        Cut,
        DeleteForward,
        DeleteLine,
        DeleteWord,
        Enter,
        IndentLine,
        MoveDocumentEnd,
        MoveDocumentStart,
        MoveDown,
        MoveLineEnd,
        MoveLineStart,
        MoveLeft,
        MoveLeftWord,
        MoveRight,
        MoveRightWord,
        MoveUp,
        Paste,
        PastePlain,
        RemoveEndDate,
        RemoveStartDate,
        SelectAll,
        SelectDocumentEnd,
        SelectDocumentStart,
        SelectDown,
        SelectLeft,
        SelectLeftWord,
        SelectLineEnd,
        SelectLineStart,
        SelectRight,
        SelectRightWord,
        SelectUp,
        SetMarkerBlank,
        SetMarkerBullet,
        SetMarkerCheckbox,
        SetMarkerNumbered,
        ShowCharacterPalette,
        ToggleBold,
        ToggleEndDate,
        ToggleHeading,
        ToggleItalic,
        ToggleRepeat,
        ToggleStartDate,
        ToggleStatus,
        UnindentLine,
    ]
);

const KEY_CONTEXT: &str = "SchemeEditor";
const MAX_INDENT: u8 = 8;
const INDENT_WIDTH: f32 = 15.0;
const TEXT_LEFT_PAD: f32 = 56.0;
const TEXT_TOP_PAD: f32 = 18.0;
const TEXT_BOTTOM_PAD: f32 = 220.0;
const TEXT_FONT_SIZE: f32 = FONT_SIZE_EDITOR;
const TEXT_LINE_HEIGHT: f32 = 20.0;
const HEADING_FONT_SIZE: f32 = 24.0;
const HEADING_LINE_HEIGHT: f32 = 30.0;
const ANNOTATION_HEIGHT: f32 = 13.0;
const ANNOTATION_FONT_SIZE: f32 = FONT_SIZE_CAPTION2;
const HANGING_WRAP_PREFIX: &str = "     ";
const HANGING_WRAP_X_OFFSET: f32 = -(CHECKBOX_SIZE + CHECKBOX_GAP);
const ANNOTATION_BAR_GAP: f32 = 6.0;
const ANNOTATION_TEXT_GAP: f32 = 5.0;
const IMAGE_TOP_GAP: f32 = 8.0;
const IMAGE_STACK_GAP: f32 = 7.0;
const IMAGE_MAX_HEIGHT: f32 = 300.0;
const IMAGE_FALLBACK_WIDTH: f32 = 320.0;
const IMAGE_FALLBACK_HEIGHT: f32 = 180.0;
const CHECKBOX_SIZE: f32 = 14.0;
const CHECKBOX_GAP: f32 = 7.0;
const EMPTY_SELECTION_WIDTH: f32 = 5.0;
const CURSOR_BLINK_DELAY: Duration = Duration::from_millis(500);
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const SCROLL_MARGIN_LINES: f32 = 4.0;
const AUTO_SCROLL_MAX_SPEED: f32 = 100.0;
const AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(8);
const AUTO_SCROLL_MIN_THRESHOLD: f32 = -15.0;
const AUTO_SCROLL_MAX_THRESHOLD: f32 = 70.0;

#[derive(Clone, Debug)]
pub enum EditorEvent {
    Command(Command),
    OpenDatePicker {
        scheme_id: SchemeId,
        item_id: ItemId,
        kind: DateKind,
        anchor: Point<Pixels>,
    },
    OpenRepeatPopover {
        scheme_id: SchemeId,
        item_id: ItemId,
        anchor: Point<Pixels>,
    },
    OpenContextMenu {
        scheme_id: SchemeId,
        item_id: ItemId,
        position: Point<Pixels>,
        date_anchor: Point<Pixels>,
    },
    CloseDatePopover,
    Focused {
        scheme_id: SchemeId,
    },
    SelectionChanged {
        scheme_id: SchemeId,
    },
}

impl EventEmitter<EditorEvent> for SchemeEditor {}

#[derive(Clone, Copy)]
struct CheckboxHitbox {
    bounds: Bounds<Pixels>,
    item_id: ItemId,
}

#[derive(Clone, Copy)]
struct DateAnnotationHitbox {
    bounds: Bounds<Pixels>,
    item_id: ItemId,
    kind: DateKind,
}

#[derive(Clone, Copy)]
struct RepeatAnnotationHitbox {
    bounds: Bounds<Pixels>,
    item_id: ItemId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseSelectionMode {
    Character,
    Word {
        anchor_start: usize,
        anchor_end: usize,
    },
    Line {
        anchor_row: usize,
    },
}

pub struct SchemeEditor {
    scheme_id: SchemeId,
    color_index: u8,
    read_only: bool,
    theme: Theme,
    time_format: TimeFormat,
    rows: Vec<EditorRow>,
    text: String,
    selection: TextSelection,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    mouse_selection_mode: Option<MouseSelectionMode>,
    cursor_blink_state: bool,
    cursor_blink_task: Option<Task<()>>,
    focus_handle: FocusHandle,
    _focus_in_subscription: Subscription,
    _focus_out_subscription: Subscription,
    line_map: LineMap,
    line_map_dirty: bool,
    pending_scroll_to_cursor: bool,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_handle: ScrollHandle,
    top_pad: f32,
    bottom_pad: f32,
    checkbox_hitboxes: Vec<CheckboxHitbox>,
    date_annotation_hitboxes: Vec<DateAnnotationHitbox>,
    repeat_annotation_hitboxes: Vec<RepeatAnnotationHitbox>,
    auto_scroll_task: Option<Task<()>>,
    auto_scroll_last_mouse_position: Option<Point<Pixels>>,
    image_cache: HashMap<Uuid, Option<Arc<Image>>>,
    /// Tracks the last auto-bulletize conversion so backspace can undo it.
    /// Stores (row, original_text, original_marker) before the conversion.
    auto_bullet_undo: Option<(usize, String, ItemMarker)>,
}

#[derive(Clone, Copy, Debug)]
pub struct EditorToolbarState {
    pub marker: ItemMarker,
    pub has_start: bool,
    pub has_end: bool,
    pub has_repeat: bool,
    pub bold: bool,
    pub italic: bool,
    pub heading: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemeEditorSessionState {
    pub anchor: TextLocation,
    pub head: TextLocation,
}

#[cfg(test)]
mod tests;
