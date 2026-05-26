use knotq_model::{ItemId, SchemeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Union,
    DailyQueue,
    Scheme,
    Settings,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub view: View,
    pub scheme_id: Option<SchemeId>,
    pub focused_item_id: Option<ItemId>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            view: View::Union,
            scheme_id: None,
            focused_item_id: None,
        }
    }
}
