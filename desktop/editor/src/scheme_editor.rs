use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    actions, div, fill, point, px, quad, relative, size, App, BorderStyle, Bounds, ClipboardItem,
    Context, Corners, CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    EventEmitter, ExternalPaths, FocusHandle, GlobalElementId, Image, IntoElement, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    PathPromptOptions, Pixels, Point, ScrollHandle, SharedString, Style, Subscription, Task,
    TextAlign, TextRun, Window,
};
use knotq_commands::{Command, DateKind};
use knotq_model::TimeFormat;
use knotq_model::{
    ImageInline, Inline, Item, ItemContent, ItemId, ItemMarker, OccurrenceId, Scheme, SchemeId,
};
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
mod links;
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
mod table;
mod text_edit;
mod utf16;

pub use keymap::init;

use annotations::*;
use buffer::*;
use clipboard::*;
use geometry::bounds_contains;
use items::*;
use links::*;
use markdown::*;
use media::*;
use navigation::*;
use selection::TextSelection;
use table::{CellSlot, TableControlHitbox, TableControlKind, TableLayout};

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
        InsertImage,
        InsertTable,
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
        ToggleHighlight,
        ToggleItalic,
        ToggleRepeat,
        ToggleStartDate,
        ToggleStatus,
        ToggleStrikethrough,
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
const HEADING2_FONT_SIZE: f32 = 21.0;
const HEADING2_LINE_HEIGHT: f32 = 27.0;
const HEADING3_FONT_SIZE: f32 = 18.0;
const HEADING3_LINE_HEIGHT: f32 = 24.0;
const ANNOTATION_HEIGHT: f32 = 13.0;
const ANNOTATION_FONT_SIZE: f32 = FONT_SIZE_CAPTION2;
const HANGING_WRAP_PREFIX: &str = "     ";
const HANGING_WRAP_X_OFFSET: f32 = -(CHECKBOX_SIZE + CHECKBOX_GAP);
const ANNOTATION_BAR_GAP: f32 = 6.0;
const ANNOTATION_TEXT_GAP: f32 = 5.0;
const INDENT_GUIDE_X_SHIFT: f32 = 2.0;
const IMAGE_TOP_GAP: f32 = 8.0;
const IMAGE_STACK_GAP: f32 = 7.0;
const IMAGE_MAX_HEIGHT: f32 = 300.0;
const IMAGE_FALLBACK_WIDTH: f32 = 320.0;
const IMAGE_FALLBACK_HEIGHT: f32 = 180.0;
const CHECKBOX_SIZE: f32 = 14.0;
const CHECKBOX_GAP: f32 = 7.0;
const EMPTY_SELECTION_WIDTH: f32 = 5.0;
const MOUSE_SELECTION_DRAG_EPSILON: f32 = 6.0;
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
    /// A detected URL was activated (Cmd/Ctrl-click, or a plain click in a
    /// read-only scheme). The host opens it in the system browser.
    OpenLink {
        scheme_id: SchemeId,
        url: String,
    },
    OpenContextMenu {
        scheme_id: SchemeId,
        item_id: ItemId,
        position: Point<Pixels>,
        date_anchor: Point<Pixels>,
        table: Option<TableContext>,
    },
    CloseDatePopover,
    Focused {
        scheme_id: SchemeId,
    },
    SelectionChanged {
        scheme_id: SchemeId,
    },
    /// A transient message for the user (e.g. an image drop/paste that was
    /// rejected). The host surfaces this however it shows notices.
    Notice {
        title: String,
        message: String,
    },
}

impl EventEmitter<EditorEvent> for SchemeEditor {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableContext {
    pub table_item_id: ItemId,
    pub row: Option<usize>,
    pub column: Option<usize>,
    pub row_count: usize,
    pub column_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableStructureAction {
    AppendRow,
    AppendColumn,
    InsertRowBefore(usize),
    InsertRowAfter(usize),
    DeleteRow(usize),
    InsertColumnBefore(usize),
    InsertColumnAfter(usize),
    DeleteColumn(usize),
}

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

#[derive(Clone)]
struct LinkHitbox {
    bounds: Bounds<Pixels>,
    url: String,
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
    editor_focused: bool,
    mouse_selection_mode: Option<MouseSelectionMode>,
    mouse_selection_origin: Option<Point<Pixels>>,
    cursor_blink_state: bool,
    cursor_blink_task: Option<Task<()>>,
    focus_handle: FocusHandle,
    _focus_in_subscription: Subscription,
    _focus_out_subscription: Subscription,
    line_map: LineMap,
    line_map_dirty: bool,
    /// Active (cursor/selection) row range used by the last layout, so a cursor
    /// move that changes which line reveals its markers can trigger a reshape.
    last_active_rows: Option<(usize, usize)>,
    pending_scroll_to_cursor: bool,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_handle: ScrollHandle,
    top_pad: f32,
    bottom_pad: f32,
    checkbox_hitboxes: Vec<CheckboxHitbox>,
    date_annotation_hitboxes: Vec<DateAnnotationHitbox>,
    repeat_annotation_hitboxes: Vec<RepeatAnnotationHitbox>,
    link_hitboxes: Vec<LinkHitbox>,
    /// The floating "open link" button shown above the link the cursor sits in,
    /// if any. Opens on a plain click (no modifier needed).
    open_link_button: Option<LinkHitbox>,
    /// Whether the mouse is currently over something openable (the link button,
    /// or a link while the secondary modifier is held). Drives the pointer cursor.
    hovered_link: bool,
    auto_scroll_task: Option<Task<()>>,
    auto_scroll_last_mouse_position: Option<Point<Pixels>>,
    image_cache: HashMap<Uuid, Option<Arc<Image>>>,
    /// Tracks the last auto-bulletize conversion so backspace can undo it.
    /// Stores (row, original_text, original_marker) before the conversion.
    auto_bullet_undo: Option<(usize, String, ItemMarker)>,
    table_layouts: HashMap<usize, TableLayout>,
    cell_slots: HashMap<usize, CellSlot>,
    table_control_hitboxes: Vec<TableControlHitbox>,
    hovered_table_control: Option<TableControlKind>,
}

#[derive(Clone, Copy, Debug)]
pub struct EditorToolbarState {
    pub marker: ItemMarker,
    pub has_start: bool,
    pub has_end: bool,
    pub has_repeat: bool,
    pub bold: bool,
    pub italic: bool,
    pub highlight: bool,
    pub strikethrough: bool,
    pub heading: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemeEditorSessionState {
    pub anchor: TextLocation,
    pub head: TextLocation,
}

#[cfg(test)]
mod tests;
