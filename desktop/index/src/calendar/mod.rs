mod build;
mod update;

use knotq_model::{ItemId, Occurrence, SchemeId};

pub use build::build_calendar_index;
pub use update::update_calendar_index;

#[derive(Clone, Debug, Default)]
pub struct CalendarIndex {
    pub items: Vec<CalendarItemContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarItemContext {
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub color_index: u8,
    pub scheme_name: String,
}

#[derive(Clone, Debug)]
pub struct OccurrenceWithContext {
    pub occurrence: Occurrence,
    pub scheme_id: SchemeId,
    pub item_id: ItemId,
    pub color_index: u8,
    pub scheme_name: String,
}
