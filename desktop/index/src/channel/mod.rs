mod build;
mod update;

use std::collections::BTreeMap;

use knotq_model::{ItemId, SchemeId};

pub use build::build_channel_index;
pub use update::update_channel_index;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChannelIndex {
    pub schemes: BTreeMap<String, SchemeId>,
    pub tasks: BTreeMap<String, ChannelTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChannelTarget {
    Scheme {
        scheme_id: SchemeId,
    },
    Item {
        scheme_id: SchemeId,
        item_id: ItemId,
    },
}

pub(crate) fn normalize_channel_ref(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('#')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
