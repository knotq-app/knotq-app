pub mod ical;

#[cfg(feature = "google")]
pub mod google;

pub use ical::{event_update_commands, map_to_commands, parse_ical, ImportedEvent};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportSource {
    Ical,
    Google,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SyncResult {
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
}
