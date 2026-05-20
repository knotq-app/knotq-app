use knotq_model::Workspace;

use crate::IndexChangeSet;

use super::{build_channel_index, ChannelIndex};

pub fn update_channel_index(
    index: &mut ChannelIndex,
    changeset: &IndexChangeSet,
    workspace: &Workspace,
) {
    if changeset.folders.is_empty() && changeset.schemes.is_empty() {
        return;
    }
    *index = build_channel_index(workspace);
}
