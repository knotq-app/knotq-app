use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_model::{DeletedSchemeOrigin, NodeRef, Scheme, Workspace};

mod support;

use support::{create_folder, create_scheme};

#[test]
fn delete_scheme_removes_all_references_to_it() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let scheme = Scheme::new("S", 1);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .extend([NodeRef::Scheme(scheme_id), NodeRef::Scheme(scheme_id)]);

    workspace
        .apply(Command::DeleteScheme { id: scheme_id })
        .unwrap();

    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(workspace.is_scheme_deleted(scheme_id));
    assert!(!workspace.folders[&root]
        .children
        .contains(&NodeRef::Scheme(scheme_id)));
}

#[test]
fn delete_scheme_records_restore_origin() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder_id = create_folder(&mut workspace, root);
    let scheme_id = create_scheme(&mut workspace, folder_id);

    let receipt = workspace
        .apply(Command::DeleteScheme { id: scheme_id })
        .unwrap();

    assert!(workspace.is_scheme_deleted(scheme_id));
    assert_eq!(
        workspace.deleted_scheme_origin(scheme_id),
        Some(DeletedSchemeOrigin {
            folder: folder_id,
            position: 0,
        })
    );

    workspace.apply(receipt.inverse).unwrap();

    assert!(!workspace.is_scheme_deleted(scheme_id));
    assert_eq!(workspace.deleted_scheme_origin(scheme_id), None);
    assert_eq!(
        workspace.folders[&folder_id].children,
        vec![NodeRef::Scheme(scheme_id)]
    );
}

#[test]
fn permanently_delete_scheme_is_undoable_to_trash() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let scheme_id = create_scheme(&mut workspace, root);

    workspace
        .apply(Command::DeleteScheme { id: scheme_id })
        .unwrap();
    let receipt = workspace
        .apply(Command::PermanentlyDeleteScheme { id: scheme_id })
        .unwrap();

    assert!(!workspace.schemes.contains_key(&scheme_id));
    assert!(!workspace.is_scheme_deleted(scheme_id));

    workspace.apply(receipt.inverse).unwrap();

    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(workspace.is_scheme_deleted(scheme_id));
    assert_eq!(
        workspace.deleted_scheme_origin(scheme_id),
        Some(DeletedSchemeOrigin {
            folder: root,
            position: 0,
        })
    );
}
