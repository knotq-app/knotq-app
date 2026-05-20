use knotq_index::channel::ChannelTarget;
use knotq_index::IndexedWorkspace;
use knotq_model::{Item, NodeRef, Scheme, Workspace};

#[test]
fn channel_query_resolves_scheme_and_task_refs() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Research", 1);
    let item = Item::new("Meet Professor");
    let item_id = item.id;
    scheme.items.push(item);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    let indexed = IndexedWorkspace::build(workspace);

    assert_eq!(
        indexed.channel_query().resolve("#Research"),
        Some(ChannelTarget::Scheme { scheme_id })
    );
    assert_eq!(
        indexed.channel_query().resolve("#Research/Meet Professor"),
        Some(ChannelTarget::Item { scheme_id, item_id })
    );
    assert_eq!(indexed.channel_query().resolve("#Research/Missing"), None);
}
