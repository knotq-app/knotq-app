use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_model::{NodeRef, Workspace};

mod support;

use support::{create_folder, create_scheme};

#[test]
fn create_and_undo_folder() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let receipt = workspace
        .apply(Command::CreateFolder {
            parent: root,
            name: "Tasks".into(),
            position: None,
        })
        .unwrap();

    assert_eq!(workspace.folders[&root].children.len(), 1);

    workspace.apply(receipt.inverse).unwrap();

    assert!(workspace.folders[&root].children.is_empty());
}

#[test]
fn delete_folder_archives_subtree_and_undo_restores_it() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder_id = create_folder(&mut workspace, root);
    let scheme_id = create_scheme(&mut workspace, folder_id);

    let delete = workspace
        .apply(Command::DeleteFolder { id: folder_id })
        .unwrap();

    assert!(workspace.folders.contains_key(&folder_id));
    assert!(workspace.is_folder_deleted(folder_id));
    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(workspace.is_scheme_deleted(scheme_id));
    assert_eq!(
        workspace.folders[&folder_id].children,
        vec![NodeRef::Scheme(scheme_id)]
    );
    assert!(!workspace.folders[&root]
        .children
        .contains(&NodeRef::Folder(folder_id)));

    workspace.apply(delete.inverse).unwrap();

    assert!(workspace.folders.contains_key(&folder_id));
    assert!(!workspace.is_folder_deleted(folder_id));
    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(!workspace.is_scheme_deleted(scheme_id));
    assert_eq!(
        workspace.folders[&folder_id].children,
        vec![NodeRef::Scheme(scheme_id)]
    );
}

#[test]
fn delete_folder_undo_redo_rearchives_restored_children() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder_id = create_folder(&mut workspace, root);
    let scheme_id = create_scheme(&mut workspace, folder_id);

    let undo = workspace
        .apply(Command::DeleteFolder { id: folder_id })
        .unwrap();
    let redo = workspace.apply(undo.inverse).unwrap();
    workspace.apply(redo.inverse).unwrap();

    assert!(workspace.folders.contains_key(&folder_id));
    assert!(workspace.is_folder_deleted(folder_id));
    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(workspace.is_scheme_deleted(scheme_id));
    assert_eq!(
        workspace.folders[&folder_id].children,
        vec![NodeRef::Scheme(scheme_id)]
    );
    assert!(!workspace.folders[&root]
        .children
        .contains(&NodeRef::Folder(folder_id)));
}
