use std::collections::HashSet;

use chrono::NaiveDate;
use knotq_commands::{Command, CommandOrigin};
use knotq_model::{
    daily_queue_scheme_id, daily_queue_sync_metadata, AppSettings, Item, ReplicaId, Scheme,
    SyncDocumentKind, Workspace,
};
use knotq_state::{WorkspaceDirtyState, WorkspaceStore};
use knotq_sync::{StoredCrdtUpdate, WorkspaceCrdtDocuments};

mod support;

use support::workspace_with_scheme_item;

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
    // CRDT reconciliation is deferred until something reads it (see
    // `WorkspaceStore::flush_crdt`); flush explicitly so the operation's
    // `crdt_updates` reflect this edit before inspecting them.
    store.flush_crdt();
    let operation = &store.pending_operations()[0];
    assert_eq!(operation.workspace_id, workspace_id);
    assert_eq!(operation.replica_id, replica_id);
    assert_eq!(operation.sequence, 1);
    assert_eq!(operation.origin, CommandOrigin::User);
    assert!(!operation.crdt_updates.is_empty());
    let crdt_update_count = operation.crdt_updates.len();

    let pending_edits = store.pending_crdt_edits();
    assert_eq!(pending_edits.len(), crdt_update_count);
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

/// A workspace with one scheme holding one editable item, plus the ids needed
/// to keep editing it.
fn editable_workspace() -> (Workspace, knotq_model::SchemeId, knotq_model::ItemId) {
    let scheme = Scheme::new("Plans", 0);
    let scheme_id = scheme.id;
    let item = Item::new("draft");
    let item_id = item.id;
    (workspace_with_scheme_item(scheme, item), scheme_id, item_id)
}

/// CRDT reconciliation is deferred (`WorkspaceStore::flush_crdt`): a burst of
/// keystrokes merges into `deferred_crdt` and only commits to the yrs documents
/// when something reads them. This must be purely a *when*, not a *what* — the
/// final document content after one coalesced flush must match what per-edit
/// reconciliation (the old behavior, still reachable by calling `flush_crdt`
/// after every edit) would have produced.
#[test]
fn deferred_burst_matches_per_edit_reconciliation() {
    let (workspace, scheme_id, item_id) = editable_workspace();

    let mut burst_store = WorkspaceStore::new::<Vec<u8>>(
        workspace.clone(),
        ReplicaId::new(),
        false,
        Default::default(),
        1,
    );
    let mut reconciled_store =
        WorkspaceStore::new::<Vec<u8>>(workspace, ReplicaId::new(), false, Default::default(), 1);

    // Seed the workspace-level CRDT document (materialization refuses to run
    // against one that has never received a workspace-kind update — see
    // `WorkspaceCrdtDocuments::apply_remote_updates`'s "local CRDT state is
    // empty" guard). A scheme rename lives in the workspace document, not the
    // scheme's own content document.
    for store in [&mut burst_store, &mut reconciled_store] {
        store
            .apply_local(
                Command::RenameScheme {
                    id: scheme_id,
                    name: "Plans v2".to_string(),
                },
                CommandOrigin::User,
            )
            .unwrap();
    }
    reconciled_store.flush_crdt();

    for i in 0..8 {
        let text = format!("line {i}");
        burst_store
            .apply_local(
                Command::UpdateItemText {
                    scheme: scheme_id,
                    item: item_id,
                    text: text.clone(),
                },
                CommandOrigin::User,
            )
            .unwrap();
        reconciled_store
            .apply_local(
                Command::UpdateItemText {
                    scheme: scheme_id,
                    item: item_id,
                    text,
                },
                CommandOrigin::User,
            )
            .unwrap();
        // Old (pre-deferral) behavior: reconcile into the CRDT after every edit.
        reconciled_store.flush_crdt();
    }

    // The burst store never flushed during the loop; this single read is where
    // all 8 deferred edits reconcile at once.
    let burst_states = burst_store.crdt_document_states();
    let reconciled_states = reconciled_store.crdt_document_states();

    let burst_docs = WorkspaceCrdtDocuments::from_states(
        burst_store.workspace(),
        ReplicaId::new(),
        &burst_states,
    )
    .unwrap();
    let reconciled_docs = WorkspaceCrdtDocuments::from_states(
        reconciled_store.workspace(),
        ReplicaId::new(),
        &reconciled_states,
    )
    .unwrap();

    let burst_materialized = burst_docs
        .materialized_workspace_for_diagnostics(burst_store.workspace())
        .unwrap();
    let reconciled_materialized = reconciled_docs
        .materialized_workspace_for_diagnostics(reconciled_store.workspace())
        .unwrap();

    let burst_text = burst_materialized.scheme(scheme_id).unwrap().items[0].text();
    let reconciled_text = reconciled_materialized.scheme(scheme_id).unwrap().items[0].text();
    assert_eq!(burst_text, "line 7");
    assert_eq!(
        burst_text, reconciled_text,
        "a coalesced flush must materialize identical content to per-edit reconciliation"
    );
}

/// `pending_crdt_edits()` after a burst of deferred edits must itself flush, and
/// the resulting updates must be a valid, self-sufficient CRDT delta: applying
/// them to a document a fresh peer has never seen reproduces the edited scheme.
#[test]
fn pending_crdt_edits_after_burst_reproduce_on_fresh_peer() {
    let (workspace, scheme_id, item_id) = editable_workspace();
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, ReplicaId::new(), false, Default::default(), 1);

    // Seed the workspace-level CRDT document — `apply_remote_updates` refuses
    // to materialize scheme content on a peer that never received a
    // workspace-kind update (see the sibling test above for why).
    store
        .apply_local(
            Command::RenameScheme {
                id: scheme_id,
                name: "Plans v2".to_string(),
            },
            CommandOrigin::User,
        )
        .unwrap();

    for i in 0..5 {
        store
            .apply_local(
                Command::UpdateItemText {
                    scheme: scheme_id,
                    item: item_id,
                    text: format!("v{i}"),
                },
                CommandOrigin::User,
            )
            .unwrap();
    }
    // Nothing has read the CRDT yet — everything above is still sitting in
    // `deferred_crdt`. `has_pending_crdt_edits` is itself a reader (it flushes
    // before answering), so this both checks and performs the coalesced flush.
    assert!(store.has_pending_crdt_edits());

    let edits = store.pending_crdt_edits();
    assert!(!edits.is_empty(), "the burst must produce a pending edit");

    let stored_updates: Vec<StoredCrdtUpdate> = edits
        .iter()
        .map(|edit| StoredCrdtUpdate {
            workspace_id: edit.workspace_id,
            document: edit.document,
            kind: edit.kind,
            replica_id: edit.replica_id,
            sequence: 0,
            received_at: chrono::Utc::now(),
            update_v1: edit.update_v1.clone(),
        })
        .collect();

    // A peer that has never seen this document before — the CRDT documents are
    // constructed empty, never seeded from `store`'s own materialized content.
    let mut fresh_peer =
        WorkspaceCrdtDocuments::empty_for_replica(store.workspace(), ReplicaId::new());
    let outcome = fresh_peer.apply_remote_updates(&Workspace::new(), &stored_updates);
    assert!(
        outcome.is_ok(),
        "fresh peer must accept the pending edits: {:?} / {:?}",
        outcome.document_errors,
        outcome.workspace_errors
    );

    let materialized_scheme = outcome
        .workspace
        .scheme(scheme_id)
        .expect("the scheme must exist on the fresh peer after applying the pending edits");
    assert_eq!(materialized_scheme.items[0].text(), "v4");
}

/// An edit that is never followed by a read (no save, no sync run, no direct
/// `flush_crdt` call) must not be lost: it stays in `deferred_crdt` until
/// something finally asks, and that later read still sees it.
#[test]
fn edit_without_a_read_is_not_lost() {
    let (workspace, scheme_id, item_id) = editable_workspace();
    let mut store =
        WorkspaceStore::new::<Vec<u8>>(workspace, ReplicaId::new(), false, Default::default(), 1);

    // A scheme rename (workspace-level) so the workspace CRDT document is
    // seeded — required for materialization below — plus a text edit
    // (scheme-level), so both kinds of deferred change are exercised.
    store
        .apply_local(
            Command::RenameScheme {
                id: scheme_id,
                name: "Renamed".to_string(),
            },
            CommandOrigin::User,
        )
        .unwrap();
    store
        .apply_local(
            Command::UpdateItemText {
                scheme: scheme_id,
                item: item_id,
                text: "only edit".to_string(),
            },
            CommandOrigin::User,
        )
        .unwrap();

    // Touch things that do NOT read the CRDT — mirrors what a keystroke burst
    // does in the app before anything asks for CRDT state (a save, a sync run).
    assert!(store.dirty().is_dirty());
    assert_eq!(store.pending_operations().len(), 2);
    assert_eq!(
        store.workspace().scheme(scheme_id).unwrap().items[0].text(),
        "only edit"
    );

    // Only now does something read the CRDT — long after the edits happened.
    let states = store.crdt_document_states();
    let docs =
        WorkspaceCrdtDocuments::from_states(store.workspace(), ReplicaId::new(), &states).unwrap();
    let materialized = docs
        .materialized_workspace_for_diagnostics(store.workspace())
        .unwrap();
    let materialized_scheme = materialized.scheme(scheme_id).unwrap();
    assert_eq!(
        materialized_scheme.name, "Renamed",
        "the workspace-level edit with no intervening read must still reach the CRDT once flushed"
    );
    assert_eq!(
        materialized_scheme.items[0].text(),
        "only edit",
        "the scheme-level edit with no intervening read must still reach the CRDT once flushed"
    );
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
    // isolation. Reconcile first: CRDT reconciliation is deferred, so the
    // setup's updates do not exist yet — draining without flushing would leave
    // them to surface later, attached to the operation for the edit probed
    // below, and the workspace update the setup legitimately made would look
    // like the content edit's.
    let _ = store.pending_crdt_edits();
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
