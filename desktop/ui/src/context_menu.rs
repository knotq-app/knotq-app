#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextMenuItem {
    Action { id: String, label: String },
    Separator,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContextMenu {
    pub items: Vec<ContextMenuItem>,
}
