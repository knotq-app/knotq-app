use knotq_commands::{Command, CommandOrigin};
use knotq_model::{AppSettings, ReplicaId, Workspace};
use knotq_state::WorkspaceStore;

#[test]
fn local_commands_are_recorded_as_pending_store_operations() {
    let workspace = Workspace::new();
    let workspace_id = workspace.id;
    let root = workspace.root;
    let replica_id = ReplicaId::new();
    let mut store = WorkspaceStore::new(workspace, replica_id, false, Default::default());

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
    let mut store = WorkspaceStore::new(workspace, replica_id, false, Default::default());

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

#[test]
fn app_settings_default_includes_replica_identity() {
    let left = AppSettings::default();
    let right = AppSettings::default();

    assert_ne!(left.replica_id, right.replica_id);
}
