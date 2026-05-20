use knotq_commands::{Command, DateKind};
use knotq_model::{ItemId, SchemeId};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug)]
pub enum EditorEvent {
    Command(Command),
    OpenDatePicker {
        scheme_id: SchemeId,
        item_id: ItemId,
        kind: DateKind,
        anchor: Option<EditorPoint>,
    },
    OpenRepeatPopover {
        scheme_id: SchemeId,
        item_id: ItemId,
        anchor: Option<EditorPoint>,
    },
    OpenContextMenu {
        scheme_id: SchemeId,
        item_id: ItemId,
        position: Option<EditorPoint>,
        date_anchor: Option<EditorPoint>,
    },
    CloseDatePopover,
    Focused {
        scheme_id: SchemeId,
    },
    SelectionChanged {
        scheme_id: SchemeId,
    },
}
