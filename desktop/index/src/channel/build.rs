use knotq_model::Workspace;

use super::{normalize_channel_ref, ChannelIndex, ChannelTarget};

pub fn build_channel_index(workspace: &Workspace) -> ChannelIndex {
    let mut index = ChannelIndex::default();
    for scheme in workspace.iter_schemes() {
        let scheme_key = normalize_channel_ref(&scheme.name);
        index.schemes.insert(scheme_key.clone(), scheme.id);
        index.tasks.insert(
            scheme_key.clone(),
            ChannelTarget::Scheme {
                scheme_id: scheme.id,
            },
        );
        for item in &scheme.items {
            let text = item.text();
            let task = text.lines().next().unwrap_or("").trim();
            if task.is_empty() {
                continue;
            }
            index.tasks.insert(
                format!("{scheme_key}/{}", normalize_channel_ref(task)),
                ChannelTarget::Item {
                    scheme_id: scheme.id,
                    item_id: item.id,
                },
            );
        }
    }
    index
}
