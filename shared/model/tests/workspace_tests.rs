use chrono::NaiveDate;
use knotq_model::{
    daily_queue_scheme_id, personal_workspace_root_folder_id, DocumentId, Folder, FolderId, Item,
    NodeRef, Scheme, SchemeId, SyncDocumentKind, Workspace, WorkspaceId, DAILY_QUEUE_COLOR_INDEX,
};

#[test]
fn normalize_removes_unreferenced_schemes_unless_recently_deleted() {
    let mut workspace = Workspace::new();
    let referenced = Scheme::new("Shown", 0);
    let referenced_id = referenced.id;
    let orphan = Scheme::new("Deleted", 1);
    let orphan_id = orphan.id;
    workspace.schemes.insert(referenced_id, referenced);
    workspace.schemes.insert(orphan_id, orphan);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(referenced_id));

    assert!(workspace.normalize_one_level_folders());
    assert!(workspace.schemes.contains_key(&referenced_id));
    assert!(!workspace.schemes.contains_key(&orphan_id));

    let mut workspace = Workspace::new();
    let deleted = Scheme::new("Deleted", 1);
    let deleted_id = deleted.id;
    workspace.schemes.insert(deleted_id, deleted);
    workspace.mark_scheme_deleted(deleted_id);

    assert!(!workspace.normalize_one_level_folders());
    assert!(workspace.schemes.contains_key(&deleted_id));
    assert!(workspace.is_scheme_deleted(deleted_id));
}

#[test]
fn normalize_preserves_nested_folders() {
    let mut workspace = Workspace::new();
    let child = FolderId::new();
    let grandchild = FolderId::new();
    let scheme = Scheme::new("Nested", 0);
    let scheme_id = scheme.id;

    workspace.folders.insert(
        child,
        Folder {
            id: child,
            name: "Child".into(),
            parent: Some(workspace.root),
            children: vec![NodeRef::Folder(grandchild)],
            expanded: true,
        },
    );
    workspace.folders.insert(
        grandchild,
        Folder {
            id: grandchild,
            name: "Grandchild".into(),
            parent: Some(child),
            children: vec![NodeRef::Scheme(scheme_id)],
            expanded: true,
        },
    );
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Folder(child));

    assert!(!workspace.normalize_one_level_folders());
    assert_eq!(
        workspace.folder(child).unwrap().children,
        vec![NodeRef::Folder(grandchild)]
    );
    assert_eq!(workspace.folder(grandchild).unwrap().parent, Some(child));
    assert!(workspace.schemes.contains_key(&scheme_id));
}

#[test]
fn normalize_keeps_trash_and_daily_queue_out_of_sidebar() {
    let mut workspace = Workspace::new();
    let active = Scheme::new("Active", 0);
    let active_id = active.id;
    let deleted = Scheme::new("Archived", 1);
    let deleted_id = deleted.id;
    let daily_date = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
    let daily_id = daily_queue_scheme_id(daily_date);
    let mut daily = Scheme::new("Daily", 0);
    daily.id = daily_id;

    workspace.schemes.insert(active_id, active);
    workspace.schemes.insert(deleted_id, deleted);
    workspace.schemes.insert(daily_id, daily);
    workspace.mark_scheme_deleted_from(deleted_id, workspace.root, 1);
    workspace.daily_queue.insert(daily_date, daily_id);
    workspace.folders.get_mut(&workspace.root).unwrap().children = vec![
        NodeRef::Scheme(active_id),
        NodeRef::Scheme(deleted_id),
        NodeRef::Scheme(daily_id),
    ];

    assert!(workspace.normalize_one_level_folders());
    assert_eq!(
        workspace.folder(workspace.root).unwrap().children,
        vec![NodeRef::Scheme(active_id)]
    );
    assert!(workspace.is_scheme_deleted(deleted_id));
    assert_eq!(workspace.daily_queue_scheme_id(daily_date), Some(daily_id));
    assert!(workspace.schemes.contains_key(&deleted_id));
    assert!(workspace.schemes.contains_key(&daily_id));
}

#[test]
fn canonical_personal_sync_identity_uses_account_workspace_id() {
    let mut workspace = Workspace::new();
    let account_workspace = WorkspaceId::new();
    let old_root = workspace.root;
    let expected_root = personal_workspace_root_folder_id(account_workspace);

    assert!(workspace.canonicalize_personal_sync_identity(account_workspace));
    assert_eq!(workspace.id, account_workspace);
    assert_eq!(workspace.sync.id, DocumentId(account_workspace.0));
    assert_eq!(workspace.sync.kind, SyncDocumentKind::PersonalWorkspace);
    assert_eq!(workspace.root, expected_root);
    assert!(workspace.folders.contains_key(&expected_root));
    assert!(!workspace.folders.contains_key(&old_root));

    assert!(!workspace.canonicalize_personal_sync_identity(account_workspace));
}

#[test]
fn canonical_personal_sync_identity_merges_duplicate_roots() {
    let account_workspace = WorkspaceId::new();
    let mut workspace = Workspace::new();
    let old_root = workspace.root;
    let local = Scheme::new("Local", 0);
    let local_id = local.id;
    let remote = Scheme::new("Remote", 1);
    let remote_id = remote.id;
    let duplicate_root = FolderId::new();

    workspace.schemes.insert(local_id, local);
    workspace.schemes.insert(remote_id, remote);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .extend([NodeRef::Scheme(local_id), NodeRef::Folder(duplicate_root)]);
    workspace.folders.insert(
        duplicate_root,
        Folder {
            id: duplicate_root,
            name: "root".into(),
            parent: None,
            children: vec![NodeRef::Scheme(remote_id)],
            expanded: true,
        },
    );

    assert!(workspace.canonicalize_personal_sync_identity(account_workspace));
    let expected_root = personal_workspace_root_folder_id(account_workspace);
    let root_children = &workspace.folder(expected_root).unwrap().children;
    assert_eq!(workspace.root, expected_root);
    assert!(root_children.contains(&NodeRef::Scheme(local_id)));
    assert!(root_children.contains(&NodeRef::Scheme(remote_id)));
    assert!(!root_children.contains(&NodeRef::Folder(old_root)));
    assert!(!root_children.contains(&NodeRef::Folder(duplicate_root)));
    assert!(!workspace.folders.contains_key(&old_root));
    assert!(!workspace.folders.contains_key(&duplicate_root));
}

#[test]
fn canonical_personal_sync_identity_migrates_legacy_daily_queue_ids() {
    let date = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
    let mut workspace = Workspace::new();
    let workspace_id = workspace.id;
    let legacy_id = SchemeId::new();
    let expected_id = daily_queue_scheme_id(date);
    let mut legacy = Scheme::new("Daily 2026-05-31", DAILY_QUEUE_COLOR_INDEX);
    legacy.id = legacy_id;
    legacy.items.push(Item::new("legacy entry"));

    workspace.schemes.insert(legacy_id, legacy);
    workspace.daily_queue.insert(date, legacy_id);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(legacy_id));

    assert!(workspace.canonicalize_personal_sync_identity(workspace_id));
    assert_eq!(workspace.daily_queue_scheme_id(date), Some(expected_id));
    assert!(!workspace.schemes.contains_key(&legacy_id));
    assert_eq!(
        workspace.schemes[&expected_id].items[0].text(),
        "legacy entry"
    );
    assert_eq!(
        workspace.scheme_sync[&expected_id].id,
        knotq_model::daily_queue_document_id(date)
    );
    assert!(!workspace
        .folder(workspace.root)
        .unwrap()
        .children
        .contains(&NodeRef::Scheme(legacy_id)));
    assert!(!workspace
        .folder(workspace.root)
        .unwrap()
        .children
        .contains(&NodeRef::Scheme(expected_id)));
}

#[test]
fn canonical_personal_sync_identity_infers_visible_daily_queue_schemes() {
    let date = NaiveDate::from_ymd_opt(2026, 5, 26).unwrap();
    let mut workspace = Workspace::new();
    let workspace_id = workspace.id;
    let legacy_id = SchemeId::new();
    let expected_id = daily_queue_scheme_id(date);
    let mut legacy = Scheme::new("Daily 2026-05-26", DAILY_QUEUE_COLOR_INDEX);
    legacy.id = legacy_id;

    workspace.schemes.insert(legacy_id, legacy);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(legacy_id));

    assert!(workspace.canonicalize_personal_sync_identity(workspace_id));
    assert_eq!(workspace.daily_queue_scheme_id(date), Some(expected_id));
    assert!(workspace.schemes.contains_key(&expected_id));
    assert!(!workspace
        .folder(workspace.root)
        .unwrap()
        .children
        .contains(&NodeRef::Scheme(legacy_id)));
}
