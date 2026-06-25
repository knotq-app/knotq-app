//! Shared helpers for the CRDT unit tests.
use super::super::*;

use knotq_model::{FolderId, NodeRef, SchemeId};

pub(super) fn add_root_folder(workspace: &mut Workspace, name: &str) -> FolderId {
    let folder = Folder {
        id: FolderId::new(),
        name: name.to_string(),
        parent: Some(workspace.root),
        children: Vec::new(),
        expanded: true,
    };
    let id = folder.id;
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Folder(id));
    workspace.folders.insert(id, folder);
    workspace.ensure_sync_metadata();
    id
}

pub(super) fn add_root_scheme(workspace: &mut Workspace, name: &str) -> SchemeId {
    let scheme = Scheme::new(name, 0);
    let id = scheme.id;
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(id));
    workspace.schemes.insert(id, scheme);
    workspace.ensure_sync_metadata();
    id
}

pub(super) fn stored_updates(
    workspace_id: knotq_model::WorkspaceId,
    updates: Vec<CrdtDocumentUpdate>,
) -> Vec<StoredCrdtUpdate> {
    updates
        .into_iter()
        .enumerate()
        .map(|(index, update)| StoredCrdtUpdate {
            workspace_id,
            document: update.document,
            kind: update.kind,
            replica_id: knotq_model::ReplicaId::new(),
            sequence: (index + 1) as u64,
            received_at: chrono::Utc::now(),
            update_v1: update.update_v1,
        })
        .collect()
}

pub(super) fn valid_single_item_scheme_doc() -> Doc {
    let doc = Doc::new();
    let metadata = doc.get_or_insert_map("scheme_file");
    let items_by_id = doc.get_or_insert_map("items_by_id");
    let scheme = Scheme::new("Plan", 0);
    let item = Item::new("First");
    let item_id = item.id.to_string();
    let mut txn = doc.transact_mut();
    metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
    metadata.insert(&mut txn, "id", scheme.id.to_string());
    let item_map = items_by_id.insert(&mut txn, item_id, MapPrelim::default());
    let snapshot_json = item_snapshot_json(&item).unwrap();
    write_new_item(&item_map, &mut txn, &item, "V", &snapshot_json).unwrap();
    drop(txn);
    doc
}

pub(super) fn encode_full_update(doc: &Doc) -> Vec<u8> {
    doc.transact().encode_diff_v1(&StateVector::default())
}
