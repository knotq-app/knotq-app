use knotq_model::Workspace;

use crate::IndexChangeSet;

use super::{build_search_index, SearchIndex};

pub fn update_search_index(
    index: &mut SearchIndex,
    changeset: &IndexChangeSet,
    workspace: &Workspace,
) {
    if changeset.folders.is_empty() && changeset.schemes.is_empty() {
        return;
    }
    *index = build_search_index(workspace);
}
