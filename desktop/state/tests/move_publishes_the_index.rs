//! One property: **moving a line to another scheme writes the workspace index.**
//!
//! A line's location is not stored anywhere — it *is* which document contains
//! the row — so a cross-scheme move is a tombstone in the source document plus
//! an insert in the destination. Neither of those is the index, and
//! `Command::crdt_documents` duly reports `workspace: false` for the batch,
//! because a move adds no scheme and removes none. That is true of the document
//! *set* and the wrong question: the index is how every other device learns the
//! destination page exists at all — its node entry, and for a Daily page its
//! queue binding.
//!
//! Leave the index out of the change set and the destination stays unpublished,
//! so the next pull materializes the account's index over it and the page, its
//! lines and its binding go with it. Production fuzz chaos 194 loses a day's
//! binding that held a line; single-account 10054 loses the row itself; chaos
//! 48, 238 and 389 are the same shape seen from the placement side.
//!
//! What is asserted here is that the move puts the index IN SCOPE, not that it
//! always rewrites it: when the index's content happens to be current, writing
//! it is a no-op, which is exactly why this costs no extra traffic for an
//! ordinary move. The cases where the content is *not* current are what the
//! fuzz seeds above cover.

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{DocumentId, Item, ItemId, NodeRef, ReplicaId, Scheme, SchemeId, Workspace};
use knotq_state::{CrdtSaveScope, WorkspaceStore};
use knotq_sync::WorkspaceCrdtDocuments;

/// Two schemes, one line each, already saved once so the next save can narrow.
fn settled_store() -> (WorkspaceStore, DocumentId, Vec<SchemeId>, Vec<ItemId>) {
    let mut workspace = Workspace::new();
    let mut schemes = Vec::new();
    let mut items = Vec::new();
    for name in ["A", "B"] {
        let mut scheme = Scheme::new(name, 0);
        let item = Item::new(format!("{name} line"));
        items.push(item.id);
        scheme.items.push(item);
        schemes.push(scheme.id);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme.id));
        workspace.schemes.insert(scheme.id, scheme);
    }
    workspace.ensure_sync_metadata();
    let index = workspace.sync.id;
    let seeded = WorkspaceCrdtDocuments::try_new(&workspace)
        .unwrap()
        .document_states();
    let mut store = WorkspaceStore::new(workspace, ReplicaId::new(), false, seeded, 1);
    let _ = store.take_crdt_save_scope();
    (store, index, schemes, items)
}

/// Whether the next save would write `document` — i.e. whether the edit put it
/// in the change set.
fn scope_covers(store: &mut WorkspaceStore, document: DocumentId) -> bool {
    match store.take_crdt_save_scope().0 {
        CrdtSaveScope::All => true,
        CrdtSaveScope::Only(named) => named.contains(&document),
    }
}

#[test]
fn moving_a_line_to_another_scheme_writes_the_workspace_index() {
    let (mut store, index, schemes, items) = settled_store();

    // The shape the command path produces for "move this line to that scheme":
    // the SAME id leaves one scheme and arrives in the other.
    let mut moved = Item::new("A line");
    moved.id = items[0];
    store
        .apply_local(
            Command::Batch(vec![
                Command::DeleteItem {
                    scheme: schemes[0],
                    item: items[0],
                },
                Command::InsertItem {
                    scheme: schemes[1],
                    position: 0,
                    item: moved,
                },
            ]),
            CommandOrigin::User,
        )
        .unwrap();

    assert!(
        scope_covers(&mut store, index),
        "a cross-scheme move left the workspace index out of the change set, so the \
         destination page is never published and the next pull materializes it away"
    );
}

/// The guard: this must not become "every edit rewrites the index". The index is
/// pushed to every device, and widening that is how a keystroke turns into
/// account-wide traffic.
#[test]
fn an_ordinary_edit_does_not_write_the_workspace_index() {
    let (mut store, index, schemes, items) = settled_store();

    store
        .apply_local(
            Command::UpdateItemText {
                scheme: schemes[0],
                item: items[0],
                text: "edited in place".to_string(),
            },
            CommandOrigin::User,
        )
        .unwrap();

    assert!(
        !scope_covers(&mut store, index),
        "an ordinary text edit put the workspace index in the change set; the index \
         is pushed to every device, so that turns a keystroke into account-wide traffic"
    );
}
