//! Repairs that re-express local scheme content must not touch the workspace
//! index or other schemes' documents.
use super::super::*;

use knotq_model::{Item, NodeRef};

fn workspace_with(names: &[&str]) -> (Workspace, Vec<SchemeId>) {
    let mut workspace = Workspace::new();
    let mut ids = Vec::new();
    for name in names {
        let mut scheme = Scheme::new(*name, 0);
        scheme.items.push(Item::new("line"));
        let id = scheme.id;
        workspace.schemes.insert(id, scheme);
        let root = workspace.root;
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(id));
        ids.push(id);
    }
    workspace.ensure_sync_metadata();
    (workspace, ids)
}

/// Production-fuzz seed 10001: the post-pull repair re-expressed one scheme
/// with the workspace from before the pull. `sync_changes` saw the schemes the
/// pull had added as "removed", re-wrote the whole index from the stale copy
/// and pruned their documents; the push then deleted those folders, schemes
/// and days on the server.
#[test]
fn a_scheme_only_sync_leaves_the_index_and_other_documents_alone() {
    let (current, ids) = workspace_with(&["Kept", "Arrived in the pull"]);
    let (kept, arrived) = (ids[0], ids[1]);
    let arrived_document = current.scheme_sync[&arrived].id;
    let mut docs = WorkspaceCrdtDocuments::try_new(&current).unwrap();

    // The repair's workspace predates the pull: it lacks the arrived scheme
    // and holds a local edit to the kept one.
    let mut stale = current.clone();
    stale.schemes.remove(&arrived);
    let root = stale.root;
    stale
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .retain(|child| *child != NodeRef::Scheme(arrived));
    stale.schemes.get_mut(&kept).unwrap().items[0].set_text("edited locally");

    let outcome = docs.sync_scheme_documents(&stale, &[kept]);
    assert!(outcome.is_ok(), "{:?}", outcome.errors);
    assert_eq!(outcome.updates.len(), 1, "only the kept scheme is written");
    assert_eq!(outcome.updates[0].kind, SyncDocumentKind::Scheme);
    assert!(
        docs.known_document_ids().contains(&arrived_document),
        "the arrived scheme's document was pruned"
    );
    let materialized = docs
        .materialized_workspace_repair(&current, &|_| false)
        .unwrap();
    assert!(materialized.schemes.contains_key(&arrived));
    assert_eq!(
        materialized.schemes[&kept].items[0].text(),
        "edited locally"
    );
}
