use knotq_index::{IndexChangeSet, IndexedWorkspace};
use knotq_model::{Item, NodeRef, Scheme, Workspace};

#[test]
fn incremental_update_matches_full_rebuild_for_changed_scheme() {
    let mut workspace = workspace_with_item("Before");
    let mut indexed = IndexedWorkspace::build(workspace.clone());
    let scheme_id = *workspace.schemes.keys().next().unwrap();
    workspace.schemes.get_mut(&scheme_id).unwrap().items[0].text = "After".to_string();
    indexed.workspace = workspace.clone();
    indexed.apply_changeset(
        &IndexChangeSet {
            folders: Vec::new(),
            schemes: vec![scheme_id],
        },
        &knotq_rrule::DefaultExpander,
    );
    let rebuilt = IndexedWorkspace::build(workspace);

    assert_eq!(indexed.search.documents, rebuilt.search.documents);
    assert_eq!(indexed.channel, rebuilt.channel);
    assert_eq!(indexed.calendar.items, rebuilt.calendar.items);
}

fn workspace_with_item(text: &str) -> Workspace {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("General", 1);
    scheme
        .items
        .push(Item::new(text).with_start(chrono::Utc::now()));
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace
}
