use knotq_model::{Item, NodeRef, Scheme, Workspace};

pub fn make_minimal_workspace() -> Workspace {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("General", 0);
    scheme.items.push(Item::new(""));
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
