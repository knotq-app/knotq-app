//! Undoing a move between schemes must leave the device satisfying the
//! projection law.
//!
//! A cross-scheme move is a delete in one document plus an insert in another,
//! so its undo is the same thing in reverse — and each half of it is a separate
//! document write. If the undo reaches the visible workspace but the documents
//! keep a live copy where the line used to be, `dedupe_materialized_items`
//! resolves the two copies by lowest scheme id and can pick the *other* one,
//! leaving the device showing a line its own documents place elsewhere
//! (production fuzz single-account seed 10193, step 101: "undo").

use chrono::Local;
use knotq_commands::Command;
use knotq_model::{AppSettings, Item, NodeRef, Scheme, Workspace};
use knotq_state::AppState;
use knotq_sync::WorkspaceCrdtDocuments;

fn state_of(workspace: &Workspace) -> AppState {
    let crdt_states = WorkspaceCrdtDocuments::try_new(workspace)
        .expect("build crdt")
        .document_states();
    let today = Local::now().date_naive();
    AppState::new(
        workspace.clone(),
        AppSettings::default(),
        today,
        today,
        false,
        crdt_states,
        0,
    )
}

#[test]
fn undoing_a_cross_scheme_move_leaves_the_device_matching_its_documents() {
    // Two schemes whose ids straddle the dedupe tie-break, which resolves by
    // lowest id — so whichever copy survives is not simply "the newest".
    let mut workspace = Workspace::new();
    let root = workspace.root;

    let mut low = Scheme::new("A", 0);
    low.id = "00000000-0000-8000-8000-000000000103".parse().unwrap();
    low.items.push(Item::new("stays in A"));

    let mut high = Scheme::new("B", 1);
    high.id = "ba929b98-afd4-80b8-ad08-c10cb3206150".parse().unwrap();
    let moved = Item::new("the travelling line");
    let moved_id = moved.id;
    high.items.push(moved.clone());

    let (low_id, high_id) = (low.id, high.id);
    workspace.schemes.insert(low_id, low);
    workspace.schemes.insert(high_id, high);
    let children = &mut workspace.folders.get_mut(&root).unwrap().children;
    children.push(NodeRef::Scheme(low_id));
    children.push(NodeRef::Scheme(high_id));
    workspace.ensure_sync_metadata();

    let mut state = state_of(&workspace);
    assert!(
        state.projection_divergences().is_empty(),
        "the fixture itself must satisfy the law"
    );

    // Move the line from the high-id scheme into the low-id one, exactly as the
    // sidebar drag does: a delete and an insert in one batch.
    state
        .apply_command(Command::Batch(vec![
            Command::DeleteItem {
                scheme: high_id,
                item: moved_id,
            },
            Command::InsertItem {
                scheme: low_id,
                position: 0,
                item: moved.clone(),
            },
        ]))
        .expect("the move applies");
    assert!(
        state.projection_divergences().is_empty(),
        "after the move: {:?}",
        state.projection_divergences()
    );

    state.undo_command().expect("the move is undoable");

    assert_eq!(
        state.workspace.schemes[&high_id]
            .items
            .iter()
            .filter(|item| item.id == moved_id)
            .count(),
        1,
        "undo puts the line back where it started"
    );
    assert!(
        !state.workspace.schemes[&low_id]
            .items
            .iter()
            .any(|item| item.id == moved_id),
        "and takes it out of where it went"
    );
    assert!(
        state.projection_divergences().is_empty(),
        "after the undo the device must still equal its own documents: {:?}",
        state.projection_divergences()
    );
}
