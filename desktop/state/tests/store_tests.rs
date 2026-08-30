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
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, replica_id, false, Default::default(), 1);

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
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, replica_id, false, Default::default(), 1);

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
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, replica_id, false, Default::default(), 1);

    let date = NaiveDate::from_ymd_opt(2026, 6, 11).unwrap();
    let id = daily_queue_scheme_id(date);
    let mut direct = store.workspace().clone();
    let mut scheme = Scheme::new("June 11", 0);
    scheme.id = id;
    scheme.items.push(Item::new("first task"));
    direct.daily_queue.insert(date, id);
    direct.schemes.insert(id, scheme);
    direct
        .scheme_sync
        .insert(id, daily_queue_sync_metadata(date));

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

/// `WorkspaceStore::after_workspace_change` only runs
/// `Workspace::ensure_sync_metadata` when the applied command could have
/// changed which schemes/folders exist (see `command_may_change_document_set`
/// in `store.rs`). A pure content edit — typing into an existing item — must
/// be skipped: it should leave sync metadata exactly as it was (already
/// current) and must not touch the workspace CRDT document, only the edited
/// scheme's.
#[test]
fn content_only_edit_skips_sync_metadata_repair_when_current() {
    let workspace = Workspace::new();
    let root = workspace.root;
    let replica_id = ReplicaId::new();
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, replica_id, false, Default::default(), 1);

    store
        .apply_local(
            Command::CreateScheme {
                folder: root,
                name: "Scheme".to_string(),
                color_index: 0,
                position: None,
            },
            CommandOrigin::User,
        )
        .unwrap();
    let scheme_id = *store.workspace().schemes.keys().next().unwrap();
    let item = Item::new("hello");
    let item_id = item.id;
    store
        .apply_local(
            Command::InsertItem {
                scheme: scheme_id,
                position: 0,
                item,
            },
            CommandOrigin::User,
        )
        .unwrap();

    assert!(
        store.workspace().sync_metadata_is_current(),
        "setup: metadata must already be current before the probed edit"
    );

    // Drain the queue so the next command's CRDT updates can be inspected in
    // isolation.
    let latest_sequence = store.pending_operations().back().unwrap().sequence;
    store.clear_pending_operations_through(latest_sequence);
    assert!(store.pending_operations().is_empty());
    let workspace_document = store.workspace().sync.id;

    store
        .apply_local(
            Command::UpdateItemText {
                scheme: scheme_id,
                item: item_id,
                text: "hello world".to_string(),
            },
            CommandOrigin::User,
        )
        .unwrap();

    assert!(
        store.workspace().sync_metadata_is_current(),
        "a content-only edit must never leave sync metadata non-current"
    );
    let edits = store.pending_crdt_edits();
    assert!(
        !edits.is_empty(),
        "the content edit must still queue an update for the edited scheme"
    );
    assert!(
        edits.iter().all(|edit| edit.document != workspace_document),
        "a content-only edit must not touch the workspace document when \
         sync metadata was already current: {edits:?}"
    );
}

/// Creating a scheme changes which schemes exist, so it must run the sync
/// metadata repair and leave the new scheme with a well-formed binding.
#[test]
fn create_scheme_repairs_sync_metadata() {
    let workspace = Workspace::new();
    let root = workspace.root;
    let replica_id = ReplicaId::new();
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, replica_id, false, Default::default(), 1);

    store
        .apply_local(
            Command::CreateScheme {
                folder: root,
                name: "Scheme".to_string(),
                color_index: 0,
                position: None,
            },
            CommandOrigin::User,
        )
        .unwrap();

    assert!(store.workspace().sync_metadata_is_current());
    let scheme_id = *store.workspace().schemes.keys().next().unwrap();
    assert!(store.workspace().scheme_sync.contains_key(&scheme_id));
}

/// Permanently deleting a scheme removes it from `workspace.schemes`, which
/// must also drop its stale `scheme_sync` binding — otherwise a later restore
/// (which mints a fresh id) would leave a dangling entry.
#[test]
fn delete_scheme_repairs_sync_metadata() {
    let workspace = Workspace::new();
    let root = workspace.root;
    let replica_id = ReplicaId::new();
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, replica_id, false, Default::default(), 1);

    store
        .apply_local(
            Command::CreateScheme {
                folder: root,
                name: "Scheme".to_string(),
                color_index: 0,
                position: None,
            },
            CommandOrigin::User,
        )
        .unwrap();
    let scheme_id = *store.workspace().schemes.keys().next().unwrap();

    store
        .apply_local(Command::DeleteScheme { id: scheme_id }, CommandOrigin::User)
        .unwrap();
    assert!(store.workspace().sync_metadata_is_current());

    store
        .apply_local(
            Command::PermanentlyDeleteScheme { id: scheme_id },
            CommandOrigin::User,
        )
        .unwrap();

    assert!(store.workspace().sync_metadata_is_current());
    assert!(!store.workspace().schemes.contains_key(&scheme_id));
    assert!(
        !store.workspace().scheme_sync.contains_key(&scheme_id),
        "the stale binding for a permanently-deleted scheme must be dropped"
    );
}

/// A long burst of content-only edits (which skip the repair) followed by a
/// structural change (which must not skip it) must still converge on
/// well-formed sync metadata — the skip must never leave a repair "owed".
#[test]
fn burst_of_content_edits_then_scheme_creation_ends_current() {
    let workspace = Workspace::new();
    let root = workspace.root;
    let replica_id = ReplicaId::new();
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, replica_id, false, Default::default(), 1);

    store
        .apply_local(
            Command::CreateScheme {
                folder: root,
                name: "Scheme".to_string(),
                color_index: 0,
                position: None,
            },
            CommandOrigin::User,
        )
        .unwrap();
    let scheme_id = *store.workspace().schemes.keys().next().unwrap();
    let item = Item::new("seed");
    let item_id = item.id;
    store
        .apply_local(
            Command::InsertItem {
                scheme: scheme_id,
                position: 0,
                item,
            },
            CommandOrigin::User,
        )
        .unwrap();

    for index in 0..50 {
        store
            .apply_local(
                Command::UpdateItemText {
                    scheme: scheme_id,
                    item: item_id,
                    text: format!("edit {index}"),
                },
                CommandOrigin::User,
            )
            .unwrap();
    }

    store
        .apply_local(
            Command::CreateScheme {
                folder: root,
                name: "Second".to_string(),
                color_index: 1,
                position: None,
            },
            CommandOrigin::User,
        )
        .unwrap();

    assert!(
        store.workspace().sync_metadata_is_current(),
        "a burst of skipped content edits must not leave a repair owed once a \
         structural command runs"
    );
}
