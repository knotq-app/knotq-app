use std::collections::HashSet;

use chrono::NaiveDate;
use knotq_commands::{Command, CommandOrigin};
use knotq_model::{
    daily_queue_scheme_id, daily_queue_sync_metadata, AppSettings, Item, ReplicaId, Scheme,
    SyncDocumentKind, Workspace,
};
use knotq_state::{WorkspaceDirtyState, WorkspaceStore};

#[test]
fn local_commands_are_recorded_as_pending_store_operations() {
    let workspace = Workspace::new();
    let workspace_id = workspace.id;
    let root = workspace.root;
    let replica_id = ReplicaId::new();
    let mut store = WorkspaceStore::new(workspace, replica_id, false, Default::default(), 1);

    let receipt = store
        .apply_local(
            Command::CreateFolder {
                parent: root,
                name: "Shared".to_string(),
                position: None,
            },
            CommandOrigin::User,
        )
        .unwrap()
        .unwrap();

    assert_eq!(receipt.touched.folders, vec![root]);
    assert!(store.dirty().is_dirty());
    assert_eq!(store.pending_operations().len(), 1);
    assert_eq!(
        store.workspace().folder_sync.len(),
        store.workspace().folders.len()
    );
    let operation = &store.pending_operations()[0];
    assert_eq!(operation.workspace_id, workspace_id);
    assert_eq!(operation.replica_id, replica_id);
    assert_eq!(operation.sequence, 1);
    assert_eq!(operation.origin, CommandOrigin::User);
    assert!(!operation.crdt_updates.is_empty());

    let pending_edits = store.pending_crdt_edits();
    assert_eq!(pending_edits.len(), operation.crdt_updates.len());
    assert!(pending_edits
        .iter()
        .all(|edit| edit.workspace_id == workspace_id && edit.replica_id == replica_id));
}

#[test]
fn acknowledged_store_operations_are_removed_in_order() {
    let workspace = Workspace::new();
    let root = workspace.root;
    let replica_id = ReplicaId::new();
    let mut store = WorkspaceStore::new(workspace, replica_id, false, Default::default(), 1);

    for name in ["A", "B"] {
        store
            .apply_local(
                Command::CreateFolder {
                    parent: root,
                    name: name.to_string(),
                    position: None,
                },
                CommandOrigin::User,
            )
            .unwrap();
    }

    assert_eq!(store.clear_pending_operations_through(1), 1);
    assert_eq!(store.pending_operations().len(), 1);
    assert_eq!(store.pending_operations()[0].sequence, 2);
}

/// Mirrors `KnotQApp::ensure_daily_queue_scheme_internal` followed by
/// `AppState::sync_store_from_workspace`: today's Daily Queue scheme is created by
/// mutating the workspace directly (no command) and reaches the store via
/// `replace_workspace`. The store must write the scheme into its CRDT and queue
/// updates that pass the server's schema validation — before this was recorded,
/// the new scheme's CRDT document stayed empty and its first push was rejected as
/// `crdt_schema_invalid`, wedging sync (production wedge of 2026-06-11).
#[test]
fn direct_daily_queue_creation_records_valid_crdt_updates() {
    let workspace = Workspace::new();
    let replica_id = ReplicaId::new();
    let mut store = WorkspaceStore::new(workspace, replica_id, false, Default::default(), 1);

    let date = NaiveDate::from_ymd_opt(2026, 6, 11).unwrap();
    let id = daily_queue_scheme_id(date);
    let mut direct = store.workspace().clone();
    let mut scheme = Scheme::new("June 11", 0);
    scheme.id = id;
    scheme.items.push(Item::new("first task"));
    direct.daily_queue.insert(date, id);
    direct.schemes.insert(id, scheme);
    direct.scheme_sync.insert(id, daily_queue_sync_metadata(date));

    store.replace_workspace(
        direct,
        WorkspaceDirtyState::from_parts(HashSet::from([id]), true),
        false,
    );

    let edits = store.pending_crdt_edits();
    assert!(!edits.is_empty(), "direct mutation must queue CRDT edits");
    let document = store.workspace().scheme_sync.get(&id).unwrap().id;
    let scheme_updates: Vec<&[u8]> = edits
        .iter()
        .filter(|edit| edit.document == document)
        .map(|edit| edit.update_v1.as_slice())
        .collect();
    assert!(
        !scheme_updates.is_empty(),
        "the new scheme document must have a CRDT update"
    );
    knotq_sync::validate_crdt_update_sequence(SyncDocumentKind::Scheme, scheme_updates)
        .expect("scheme update must pass server schema validation");

    // Replaying the same workspace with the same dirty set must not re-emit
    // duplicate updates — the CRDT already holds the content.
    let direct = store.workspace().clone();
    let before = store.pending_crdt_edits().len();
    store.replace_workspace(
        direct,
        WorkspaceDirtyState::from_parts(HashSet::from([id]), true),
        false,
    );
    assert_eq!(
        store.pending_crdt_edits().len(),
        before,
        "an unchanged workspace must not queue new CRDT edits"
    );
}

#[test]
fn app_settings_default_includes_replica_identity() {
    let left = AppSettings::default();
    let right = AppSettings::default();

    assert_ne!(left.replica_id, right.replica_id);
}
