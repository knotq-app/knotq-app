//! Workspace/scheme materialization and remote-update CRDT unit tests.
use super::super::*;

use super::helpers::{add_root_folder, add_root_scheme, stored_updates};
use chrono::NaiveDate;
use knotq_model::{Item, NodeRef};

#[test]
fn workspace_crdt_documents_emit_scheme_updates_for_touched_schemes() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Plan", 0);
    scheme.items.push(Item::new("First"));
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace.ensure_sync_metadata();

    let mut docs = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
    workspace.schemes.get_mut(&scheme_id).unwrap().items[0].set_text("Changed");
    let updates = docs
        .sync_changes(
            &workspace,
            &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
        )
        .updates;

    assert!(updates
        .iter()
        .any(|update| update.kind == SyncDocumentKind::Scheme));
}

#[test]
fn remote_crdt_updates_materialize_workspace_and_scheme_items() {
    let mut source = Workspace::new();
    let mut scheme = Scheme::new("Remote Plan", 2);
    scheme.items.push(Item::new("First remote line"));
    let scheme_id = scheme.id;
    source
        .folders
        .get_mut(&source.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    source.schemes.insert(scheme_id, scheme);
    source.ensure_sync_metadata();

    let updates = WorkspaceCrdtDocuments::snapshot_updates(&source)
        .updates
        .into_iter()
        .enumerate()
        .map(|(index, update)| StoredCrdtUpdate {
            workspace_id: source.id,
            document: update.document,
            kind: update.kind,
            replica_id: knotq_model::ReplicaId::new(),
            sequence: (index + 1) as u64,
            received_at: chrono::Utc::now(),
            update_v1: update.update_v1,
        })
        .collect::<Vec<_>>();

    let mut target = source.clone();
    target.schemes.get_mut(&scheme_id).unwrap().items.clear();
    let mut docs = WorkspaceCrdtDocuments::try_new(&target).unwrap();
    let outcome = docs.apply_remote_updates(&target, &updates);

    assert!(outcome.is_ok(), "{:?}", outcome.document_errors);
    assert_eq!(
        outcome.workspace.schemes[&scheme_id].items[0].text(),
        "First remote line"
    );
    assert!(outcome.workspace.folders[&outcome.workspace.root]
        .children
        .contains(&NodeRef::Scheme(scheme_id)));
}

#[test]
fn reidentified_workspace_merges_local_and_server_schemes_without_mismatch() {
    // The server account's canonical personal-workspace document (id B) holds
    // its own scheme. Mirror the server-side canonicalization so its document
    // id is DocumentId(workspace_id).
    let mut server = Workspace::new();
    let server_scheme = add_root_scheme(&mut server, "ServerScheme");
    server.canonicalize_personal_sync_identity(server.id);
    let server_doc_id = server.sync.id;
    let server_workspace_update: Vec<CrdtDocumentUpdate> =
        WorkspaceCrdtDocuments::snapshot_updates(&server)
            .updates
            .into_iter()
            .filter(|update| update.kind == SyncDocumentKind::PersonalWorkspace)
            .collect();

    // This device's workspace (a different account, id A) holds its own scheme.
    let mut local = Workspace::new();
    let local_scheme = add_root_scheme(&mut local, "LocalScheme");
    local.canonicalize_personal_sync_identity(local.id);
    assert_ne!(local.sync.id, server_doc_id);

    // Applying the server's workspace update as-is fails with a document-id
    // mismatch (the bug): the local CRDT workspace doc still carries id A.
    let mut before = WorkspaceCrdtDocuments::try_new(&local).unwrap();
    let before_outcome = before
        .apply_remote_updates(&local, &stored_updates(server.id, server_workspace_update.clone()));
    assert!(
        !before_outcome.workspace_is_ok(),
        "expected a document-id mismatch before re-identify"
    );

    // Re-identify the local workspace doc to the server's id (preserving its
    // content) and adopt the server account identity on the materialized
    // workspace, then the same update applies cleanly and unions both schemes.
    let mut docs = WorkspaceCrdtDocuments::try_new(&local).unwrap();
    let snapshot = docs.reidentify_workspace_document(server_doc_id).unwrap();
    assert!(snapshot.is_some(), "re-identify should report the relabeled doc");
    local.canonicalize_personal_sync_identity(server.id);
    let outcome =
        docs.apply_remote_updates(&local, &stored_updates(server.id, server_workspace_update));
    assert!(outcome.workspace_is_ok(), "{:?}", outcome.workspace_errors);
    assert!(
        outcome.workspace.schemes.contains_key(&local_scheme),
        "local scheme preserved through the merge"
    );
    assert!(
        outcome.workspace.schemes.contains_key(&server_scheme),
        "server scheme merged in over the shared id"
    );
}

#[test]
fn remote_workspace_materialization_keeps_trash_and_daily_queue_out_of_sidebar() {
    let mut source = Workspace::new();
    let active = add_root_scheme(&mut source, "Active");

    let archived = Scheme::new("Archived", 1);
    let archived_id = archived.id;
    source.schemes.insert(archived_id, archived);
    source.mark_scheme_deleted_from(archived_id, source.root, 0);

    let daily_date = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
    let daily_id = knotq_model::daily_queue_scheme_id(daily_date);
    let mut daily = Scheme::new("Daily", 0);
    daily.id = daily_id;
    source.daily_queue.insert(daily_date, daily_id);
    source.schemes.insert(daily_id, daily);
    source.ensure_sync_metadata();

    let updates = WorkspaceCrdtDocuments::snapshot_updates(&source).updates;
    let mut target = Workspace::new();
    target.id = source.id;
    target.sync = source.sync.clone();
    target.root = source.root;
    target.folders.insert(
        source.root,
        Folder {
            id: source.root,
            name: "root".to_string(),
            parent: None,
            children: Vec::new(),
            expanded: true,
        },
    );
    let mut docs = WorkspaceCrdtDocuments::empty(&target);
    let outcome = docs.apply_remote_updates(&target, &stored_updates(source.id, updates));

    assert!(outcome.is_ok(), "{:?}", outcome.document_errors);
    let root_children = &outcome.workspace.folders[&outcome.workspace.root].children;
    assert!(root_children.contains(&NodeRef::Scheme(active)));
    assert!(!root_children.contains(&NodeRef::Scheme(archived_id)));
    assert!(!root_children.contains(&NodeRef::Scheme(daily_id)));
    assert!(outcome.workspace.is_scheme_deleted(archived_id));
    assert_eq!(
        outcome.workspace.daily_queue_scheme_id(daily_date),
        Some(daily_id)
    );
}

#[test]
fn incremental_archive_update_keeps_scheme_out_of_sidebar() {
    let mut source = Workspace::new();
    let scheme = add_root_scheme(&mut source, "Archive Me");
    // One persistent document produces both the initial snapshot and the
    // later archive delta, exactly as the client's long-lived Store does.
    // Emitting them from two different documents would give the same logical
    // state two different Yjs identities and break LWW convergence.
    let mut source_docs = WorkspaceCrdtDocuments::empty(&source);
    let initial_updates = source_docs
        .sync_changes(&source, &WorkspaceCrdtChangeSet::default().workspace())
        .updates;

    let mut target = Workspace::new();
    target.id = source.id;
    target.sync = source.sync.clone();
    target.root = source.root;
    let mut target_docs = WorkspaceCrdtDocuments::empty(&target);
    let initial =
        target_docs.apply_remote_updates(&target, &stored_updates(source.id, initial_updates));
    assert!(initial.is_ok(), "{:?}", initial.document_errors);
    target = initial.workspace;
    assert!(target
        .folder(target.root)
        .unwrap()
        .children
        .contains(&NodeRef::Scheme(scheme)));

    source
        .folders
        .get_mut(&source.root)
        .unwrap()
        .children
        .retain(|child| *child != NodeRef::Scheme(scheme));
    source.mark_scheme_deleted_from(scheme, source.root, 0);
    let archive_updates = source_docs
        .sync_changes(&source, &WorkspaceCrdtChangeSet::default().workspace())
        .updates;
    assert!(
        !archive_updates.is_empty(),
        "archive should emit a workspace update"
    );

    let archived =
        target_docs.apply_remote_updates(&target, &stored_updates(source.id, archive_updates));
    assert!(archived.is_ok(), "{:?}", archived.document_errors);
    assert!(archived.workspace.is_scheme_deleted(scheme));
    assert!(!archived
        .workspace
        .folder(archived.workspace.root)
        .unwrap()
        .children
        .contains(&NodeRef::Scheme(scheme)));
}

#[test]
fn workspace_crdt_documents_emit_workspace_updates_for_removed_schemes() {
    let mut workspace = Workspace::new();
    let scheme = Scheme::new("Plan", 0);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace.mark_scheme_deleted(scheme_id);
    workspace.ensure_sync_metadata();

    let mut docs = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
    workspace.schemes.remove(&scheme_id);
    workspace.recently_deleted.retain(|id| *id != scheme_id);
    workspace.ensure_sync_metadata();

    let updates = docs
        .sync_changes(
            &workspace,
            &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
        )
        .updates;

    assert!(updates
        .iter()
        .any(|update| update.kind == SyncDocumentKind::PersonalWorkspace));
}

#[test]
fn folder_changes_emit_workspace_document_not_folder_documents() {
    let mut workspace = Workspace::new();
    let folder = Folder {
        id: FolderId::new(),
        name: "Projects".to_string(),
        parent: Some(workspace.root),
        children: Vec::new(),
        expanded: true,
    };
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Folder(folder.id));
    workspace.folders.insert(folder.id, folder);
    workspace.ensure_sync_metadata();

    let mut docs = WorkspaceCrdtDocuments::empty(&workspace);
    let updates = docs
        .sync_changes(&workspace, &WorkspaceCrdtChangeSet::default().workspace())
        .updates;

    assert!(updates
        .iter()
        .any(|update| update.kind == SyncDocumentKind::PersonalWorkspace));
    assert!(!updates
        .iter()
        .any(|update| update.kind == SyncDocumentKind::Folder));
}

#[test]
fn concurrent_folder_additions_on_two_replicas_merge_without_loss() {
    // A shared base workspace that both replicas start from.
    let base = Workspace::new();

    // Each replica adds a different folder under the root and pushes its full
    // document state, exactly as a first-time/bootstrap sync does.
    let mut workspace_a = base.clone();
    let folder_x = add_root_folder(&mut workspace_a, "X");
    let a_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_a).updates;

    let mut workspace_b = base.clone();
    let folder_y = add_root_folder(&mut workspace_b, "Y");
    let b_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_b).updates;

    // The server holds the base and merges both replicas' deltas.
    let mut server = WorkspaceCrdtDocuments::try_new(&base).unwrap();
    let outcome_a = server.apply_remote_updates(&base, &stored_updates(base.id, a_updates));
    assert!(outcome_a.is_ok(), "{:?}", outcome_a.document_errors);
    let outcome_b =
        server.apply_remote_updates(&outcome_a.workspace, &stored_updates(base.id, b_updates));
    assert!(outcome_b.is_ok(), "{:?}", outcome_b.document_errors);

    // Both concurrently-added folders survive — neither clobbers the other the
    // way a single whole-document last-writer-wins blob would.
    let merged = outcome_b.workspace;
    assert!(merged.folders.contains_key(&folder_x), "folder X lost");
    assert!(merged.folders.contains_key(&folder_y), "folder Y lost");
    let root_children = &merged.folders[&merged.root].children;
    assert!(root_children.contains(&NodeRef::Folder(folder_x)));
    assert!(root_children.contains(&NodeRef::Folder(folder_y)));
}

#[test]
fn concurrent_scheme_additions_under_root_merge_without_loss() {
    let base = Workspace::new();

    let mut workspace_a = base.clone();
    let scheme_a = add_root_scheme(&mut workspace_a, "A");
    let a_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_a).updates;

    let mut workspace_b = base.clone();
    let scheme_b = add_root_scheme(&mut workspace_b, "B");
    let b_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_b).updates;

    let mut server = WorkspaceCrdtDocuments::try_new(&base).unwrap();
    let outcome_a = server.apply_remote_updates(&base, &stored_updates(base.id, a_updates));
    assert!(outcome_a.is_ok(), "{:?}", outcome_a.document_errors);
    let outcome_b =
        server.apply_remote_updates(&outcome_a.workspace, &stored_updates(base.id, b_updates));
    assert!(outcome_b.is_ok(), "{:?}", outcome_b.document_errors);

    let merged = outcome_b.workspace;
    assert!(merged.schemes.contains_key(&scheme_a), "scheme A lost");
    assert!(merged.schemes.contains_key(&scheme_b), "scheme B lost");
    let root_children = &merged.folders[&merged.root].children;
    assert!(root_children.contains(&NodeRef::Scheme(scheme_a)));
    assert!(root_children.contains(&NodeRef::Scheme(scheme_b)));
}
