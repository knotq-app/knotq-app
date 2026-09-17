//! Landing a sync run must not clear an edit recorded while the run was in flight.
//!
//! A run queues edits of its own (a first sync's re-keyed workspace, a repair,
//! a bootstrap snapshot) under sequences past the store's and pushes through
//! them, so a pushed `through_local_sequence` says nothing about which store
//! operations were sent. Only operations the run's snapshot held — sequenced
//! below the snapshot watermark — can have been.

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{Item, NodeRef, ReplicaId, Scheme, Workspace};
use knotq_state::WorkspaceStore;
use knotq_sync::WorkspaceCrdtDocuments;

#[test]
fn landing_keeps_an_edit_made_after_the_runs_snapshot_even_at_a_pushed_sequence() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Plans", 0);
    let item = Item::new("a line");
    let item_id = item.id;
    let scheme_id = scheme.id;
    scheme.items.push(item);
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
    let mut store = WorkspaceStore::new(workspace, ReplicaId::new(), false, seeded, 1);
    let document = store.workspace().scheme_sync[&scheme_id].id;
    let retype = |store: &mut WorkspaceStore, text: &str| {
        store
            .apply_local(
                Command::UpdateItemText {
                    scheme: scheme_id,
                    item: item_id,
                    text: text.to_string(),
                },
                CommandOrigin::User,
            )
            .unwrap();
    };

    // An edit the run carries, then the run's snapshot.
    retype(&mut store, "typed before the sync");
    let _snapshot = store.pending_crdt_edits();
    let watermark = store.local_sequence_watermark();

    // An edit while the run is in flight.
    retype(&mut store, "typed during the sync");
    let _ = store.pending_crdt_edits();

    // The run lands having pushed this document well past the store's sequences.
    store.clear_pushed_crdt_edits(document, watermark + 10, watermark);

    let still_queued: Vec<u64> = store
        .pending_operations()
        .iter()
        .filter(|operation| {
            operation
                .crdt_updates
                .iter()
                .any(|update| update.document == document)
        })
        .map(|operation| operation.sequence)
        .collect();
    assert_eq!(
        still_queued,
        vec![watermark],
        "landing must clear the snapshot's edit and keep the one made during the sync"
    );
}
