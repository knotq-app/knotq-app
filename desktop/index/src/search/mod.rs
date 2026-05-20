mod build;
mod tokenize;
mod update;

use knotq_model::{ItemId, SchemeId};

pub use build::build_search_index;
pub use tokenize::tokenize;
pub use update::update_search_index;

pub use crate::query::search::{
    search_hits, SearchHit, SearchHitStatus, SearchOptions, SearchTarget,
};

#[derive(Clone, Debug, Default)]
pub struct SearchIndex {
    pub documents: Vec<SearchDocument>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchDocument {
    pub scheme_id: SchemeId,
    pub item_id: Option<ItemId>,
    pub text: String,
    pub tokens: Vec<String>,
}
