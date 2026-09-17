//! A sync run whose workspace index binds a scheme to a content document the
//! store has never held.
//!
//! Another device can re-create a scheme's content document. Landing merged the
//! run's states into the documents the store already had, adopted the new
//! binding, and never built the new document — so the scheme kept materializing
//! from its old plain copy while every later sync reported a change (deep
//! production fuzz, seed 10004: lines deleted on every other device stayed on
//! one device forever).

use knotq_model::{DocumentId, Item, NodeRef, ReplicaId, Scheme, Workspace};
use knotq_state::WorkspaceStore;
use knotq_sync::WorkspaceCrdtDocuments;

#[test]
fn a_scheme_rebound_to_a_new_document_lands_its_content() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Coursework", 0);
    let scheme_id = scheme.id;
    let lines: Vec<Item> = ["first", "second", "third"]
        .into_iter()
        .map(Item::new)
        .collect();
    let removed = lines[1].id;
    scheme.items = lines;
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace.schemes.insert(scheme_id, scheme);
    workspace.ensure_sync_metadata();
    let seeded = WorkspaceCrdtDocuments::try_new(&workspace)
        .unwrap()
        .document_states();
    let mut store = WorkspaceStore::new(workspace.clone(), ReplicaId::new(), false, seeded, 1);

    // The account's index now binds the scheme to a different content document,
    // which no longer holds the second line.
    let mut remote = workspace.clone();
    remote
        .schemes
        .get_mut(&scheme_id)
        .unwrap()
        .items
        .retain(|item| item.id != removed);
    remote.scheme_sync.get_mut(&scheme_id).unwrap().id = DocumentId::new();
    let states = WorkspaceCrdtDocuments::try_new(&remote)
        .unwrap()
        .document_states();

    // Landing, as `AppState::replace_workspace_from_sync` does it.
    if !store.merge_sync_crdt_states(&remote, &states) {
        store.replace_from_sync(remote.clone(), states);
    }

    let landed: Vec<_> = store
        .workspace()
        .scheme(scheme_id)
        .expect("scheme")
        .items
        .iter()
        .map(|item| item.id)
        .collect();
    let expected: Vec<_> = remote
        .scheme(scheme_id)
        .unwrap()
        .items
        .iter()
        .map(|item| item.id)
        .collect();
    assert_eq!(landed, expected, "the store kept the old document's lines");
    assert_eq!(
        store.workspace().scheme_sync[&scheme_id].id,
        remote.scheme_sync[&scheme_id].id
    );
}
