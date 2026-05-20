use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_model::{NodeRef, Workspace};

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
fn delete_folder_removes_child_schemes_and_undo_restores_them() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder_id = create_folder(&mut workspace, root);
    let scheme_id = create_scheme(&mut workspace, folder_id);

    let delete = workspace
        .apply(Command::DeleteFolder { id: folder_id })
        .unwrap();

    assert!(!workspace.folders.contains_key(&folder_id));
    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(workspace.is_scheme_deleted(scheme_id));

    workspace.apply(delete.inverse).unwrap();

    assert!(workspace.folders.contains_key(&folder_id));
    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(!workspace.is_scheme_deleted(scheme_id));
    assert_eq!(
        workspace.folders[&folder_id].children,
        vec![NodeRef::Scheme(scheme_id)]
    );
}

#[test]
fn delete_folder_undo_redo_removes_restored_children() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder_id = create_folder(&mut workspace, root);
    let scheme_id = create_scheme(&mut workspace, folder_id);

    let undo = workspace
        .apply(Command::DeleteFolder { id: folder_id })
        .unwrap();
    let redo = workspace.apply(undo.inverse).unwrap();
    workspace.apply(redo.inverse).unwrap();

    assert!(!workspace.folders.contains_key(&folder_id));
    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(workspace.is_scheme_deleted(scheme_id));
    assert!(!workspace.folders[&root]
        .children
        .contains(&NodeRef::Folder(folder_id)));
}

fn create_folder(
    workspace: &mut Workspace,
    parent: knotq_model::FolderId,
) -> knotq_model::FolderId {
    let receipt = workspace
        .apply(Command::CreateFolder {
            parent,
            name: "Projects".into(),
            position: None,
        })
        .unwrap();
    match receipt.inverse {
        Command::DeleteFolder { id } => id,
        _ => unreachable!(),
    }
}

fn create_scheme(
    workspace: &mut Workspace,
    folder: knotq_model::FolderId,
) -> knotq_model::SchemeId {
    let receipt = workspace
        .apply(Command::CreateScheme {
            folder,
            name: "S".into(),
            color_index: 1,
            position: None,
        })
        .unwrap();
    match receipt.inverse {
        Command::DeleteScheme { id } => id,
        Command::Batch(commands) => commands
            .into_iter()
            .find_map(|command| match command {
                Command::DeleteScheme { id } => Some(id),
                _ => None,
            })
            .unwrap(),
        _ => unreachable!(),
    }
}
