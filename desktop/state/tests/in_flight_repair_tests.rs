//! Edits recorded while a sync run is in flight must survive that run's landing.
//!
//! Landing clears what the run pushed by sequence, and only operations the run's
//! snapshot held (sequenced below the snapshot watermark) can have been pushed.
//! So every CRDT update produced after the snapshot has to live on an operation
//! sequenced at or above the watermark — never be folded into an older one.

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{FolderId, Item, NodeRef, ReplicaId, Scheme, Workspace};
use knotq_state::WorkspaceStore;
use knotq_sync::WorkspaceCrdtDocuments;

#[test]
fn an_index_repair_made_while_a_sync_is_in_flight_survives_its_landing() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Plans", 0);
    let item = Item::new("a line");
    let item_id = item.id;
    scheme.items.push(item);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme.id));
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme.id, scheme);
    // A trash entry with no folder behind it: `repair_workspace_index` drops it.
    workspace.recently_deleted_folders.push(FolderId::new());
    workspace.ensure_sync_metadata();
    let seeded = WorkspaceCrdtDocuments::try_new(&workspace)
        .unwrap()
        .document_states();
    let mut store = WorkspaceStore::new(workspace, ReplicaId::new(), false, seeded, 1);
    let index_document = store.workspace().sync.id;

    // An edit the run will carry, then the run's snapshot.
    store
        .apply_local(
            Command::UpdateItemText {
                scheme: scheme_id,
                item: item_id,
                text: "edited before the sync".to_string(),
            },
            CommandOrigin::User,
        )
        .unwrap();
    let _snapshot = store.pending_crdt_edits();
    let watermark = store.local_sequence_watermark();

    // While the run is in flight: an index repair, and no command after it.
    assert!(
        store.repair_workspace_index(),
        "the dangling trash entry was repaired"
    );

    // The run lands having pushed the index document well past the store's own
    // sequences (it queues edits of its own).
    store.clear_pushed_crdt_edits(index_document, watermark + 10, watermark);

    let repair_still_queued = store
        .pending_operations()
        .iter()
        .flat_map(|operation| operation.crdt_updates.iter())
        .any(|update| update.document == index_document);
    assert!(
        repair_still_queued,
        "the index repair made during the sync was cleared as pushed without ever being sent"
    );
}
