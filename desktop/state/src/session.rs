use std::collections::HashMap;

use knotq_model::{ItemId, SchemeId};

#[derive(Clone, Debug, Default)]
pub struct EditorSession {
    pub focused_item_id: Option<ItemId>,
    pub scroll_y: f32,
    pub menu: SchemeEditorMenuState,
    pub dirty: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SchemeEditorMenuState {
    #[default]
    None,
    DatePicker {
        item_id: ItemId,
    },
    ContextMenu {
        item_id: ItemId,
    },
}

pub type EditorSessions = HashMap<SchemeId, EditorSession>;
