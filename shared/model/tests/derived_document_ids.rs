//! Every scheme/folder/daily-queue → document binding must be *derived*, never
//! randomly minted.
//!
//! `ensure_sync_metadata` re-mints a binding whenever one is missing, which
//! happens independently on each device (a node created locally, or a binding
//! transiently dropped while its node is absent from a materialized workspace).
//! A random mint therefore lets two devices bind the SAME node to DIFFERENT
//! documents; the divergent bindings race in the workspace-document merge and
//! the loser's document becomes unreachable — silent content loss, and the
//! reason `scheme_content_document_id` exists.
//!
//! Folders were missed when schemes were fixed, so these tests assert the
//! property for every node kind rather than only the one that had a bug.

use knotq_model::{
    daily_queue_document_id, daily_queue_scheme_id, folder_document_id, scheme_content_document_id,
    Folder, FolderId, Item, NodeRef, Scheme, Workspace,
};

use chrono::NaiveDate;

fn workspace_with_a_folder_and_scheme() -> (Workspace, FolderId, Scheme) {
    let mut workspace = Workspace::new();
    let folder = Folder {
        id: FolderId::new(),
        name: "folder".into(),
        parent: Some(workspace.root),
        children: Vec::new(),
        expanded: true,
    };
    let folder_id = folder.id;
    workspace.folders.insert(folder_id, folder);

    let mut scheme = Scheme::new("scheme", 0);
    scheme.items = vec![Item::new("alpha")];
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme.id));
    workspace.schemes.insert(scheme.id, scheme.clone());
    (workspace, folder_id, scheme)
}

/// Two devices that independently mint a binding for the same folder must land
/// on the same document. This is the folder analogue of
/// `scheme_binding_remint_is_deterministic_and_convergent`.
#[test]
fn independent_devices_mint_the_same_folder_document() {
    let (base, folder_id, _) = workspace_with_a_folder_and_scheme();

    let mut device_a = base.clone();
    device_a.folder_sync.remove(&folder_id);
    device_a.ensure_sync_metadata();

    let mut device_b = base.clone();
    device_b.folder_sync.remove(&folder_id);
    device_b.ensure_sync_metadata();

    let bound_a = device_a.folder_sync[&folder_id].id;
    let bound_b = device_b.folder_sync[&folder_id].id;
    assert_eq!(
        bound_a, bound_b,
        "two devices minted different documents for the same folder — the \
         divergent bindings race in the workspace merge and orphan one side"
    );
    assert_eq!(
        bound_a,
        folder_document_id(folder_id),
        "the folder binding must be the derived id, not a random one"
    );
}

/// A binding dropped and re-minted must come back identical, so a transient
/// absence never rebinds the folder to a fresh document.
#[test]
fn a_folder_binding_survives_being_dropped_and_re_minted() {
    let (mut workspace, folder_id, _) = workspace_with_a_folder_and_scheme();
    workspace.ensure_sync_metadata();
    let original = workspace.folder_sync[&folder_id].id;

    // The folder momentarily vanishes (the `retain` drops its binding) and
    // returns.
    let folder = workspace.folders.remove(&folder_id).unwrap();
    workspace.ensure_sync_metadata();
    assert!(!workspace.folder_sync.contains_key(&folder_id));
    workspace.folders.insert(folder_id, folder);
    workspace.ensure_sync_metadata();

    assert_eq!(
        workspace.folder_sync[&folder_id].id, original,
        "a re-mint after transient absence must rebuild the identical binding"
    );
}

/// The same property for schemes and daily-queue days, so a future refactor
/// cannot regress one while leaving the others intact.
#[test]
fn every_node_kind_binds_to_a_derived_document() {
    let (mut workspace, folder_id, scheme) = workspace_with_a_folder_and_scheme();
    let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
    let daily_id = daily_queue_scheme_id(date);
    let mut daily = Scheme::new("Daily", 0);
    daily.id = daily_id;
    workspace.schemes.insert(daily_id, daily);
    workspace.daily_queue.insert(date, daily_id);
    workspace.ensure_sync_metadata();

    assert_eq!(
        workspace.folder_sync[&folder_id].id,
        folder_document_id(folder_id)
    );
    assert_eq!(
        workspace.scheme_sync[&scheme.id].id,
        scheme_content_document_id(scheme.id)
    );
    assert_eq!(
        workspace.scheme_sync[&daily_id].id,
        daily_queue_document_id(date)
    );
}

/// Derived ids must not collide across namespaces: a folder and a scheme that
/// happen to share a uuid must still get distinct documents, or one would
/// silently overwrite the other.
#[test]
fn derived_document_namespaces_do_not_collide() {
    let raw = uuid::Uuid::new_v4();
    let folder = FolderId(raw);
    let scheme = knotq_model::SchemeId(raw);
    assert_ne!(
        folder_document_id(folder).0,
        scheme_content_document_id(scheme).0,
        "folder and scheme document namespaces collide"
    );

    let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
    let daily_scheme = daily_queue_scheme_id(date);
    assert_ne!(
        daily_queue_document_id(date).0,
        scheme_content_document_id(daily_scheme).0,
        "a daily-queue day's document must not collide with the generic \
         scheme-content derivation for the same id"
    );
}

/// Mint-on-absence only: an existing binding — including a legacy random one
/// from before the derivation existed — must never be silently rewritten, or an
/// upgrade would rebind every folder and orphan its document.
#[test]
fn an_existing_binding_is_never_rewritten() {
    let (mut workspace, folder_id, _) = workspace_with_a_folder_and_scheme();
    workspace.ensure_sync_metadata();

    let legacy = knotq_model::DocumentId::new();
    workspace.folder_sync.get_mut(&folder_id).unwrap().id = legacy;
    assert!(
        !workspace.ensure_sync_metadata(),
        "a workspace whose only oddity is a legacy folder document id needs no repair"
    );
    assert_eq!(
        workspace.folder_sync[&folder_id].id, legacy,
        "an existing folder binding must be preserved across the upgrade"
    );
}
