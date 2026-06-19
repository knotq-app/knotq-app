#![allow(dead_code)]

use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_model::{FolderId, SchemeId, Workspace};

pub fn create_folder(workspace: &mut Workspace, parent: FolderId) -> FolderId {
    create_folder_named(workspace, parent, "Projects")
}

pub fn create_folder_named(
    workspace: &mut Workspace,
    parent: FolderId,
    name: impl Into<String>,
) -> FolderId {
    let receipt = workspace
        .apply(Command::CreateFolder {
            parent,
            name: name.into(),
            position: None,
        })
        .unwrap();
    folder_id_from_inverse(receipt.inverse)
}

pub fn create_root_scheme(workspace: &mut Workspace) -> SchemeId {
    create_scheme(workspace, workspace.root)
}

pub fn create_scheme(workspace: &mut Workspace, folder: FolderId) -> SchemeId {
    create_scheme_named(workspace, folder, "S", 1)
}

pub fn create_scheme_named(
    workspace: &mut Workspace,
    folder: FolderId,
    name: impl Into<String>,
    color_index: u8,
) -> SchemeId {
    let receipt = workspace
        .apply(Command::CreateScheme {
            folder,
            name: name.into(),
            color_index,
            position: None,
        })
        .unwrap();
    scheme_id_from_inverse(receipt.inverse)
}

pub fn roundtrip(workspace: &mut Workspace, command: Command, original: serde_json::Value) {
    let receipt = workspace.apply(command).unwrap();
    workspace.apply(receipt.inverse).unwrap();
    assert_eq!(snapshot(workspace), original);
}

pub fn snapshot(workspace: &Workspace) -> serde_json::Value {
    serde_json::to_value(workspace).unwrap()
}

fn folder_id_from_inverse(command: Command) -> FolderId {
    match command {
        Command::DeleteFolder { id } => id,
        Command::Batch(commands) => commands
            .into_iter()
            .find_map(|command| match command {
                Command::DeleteFolder { id } => Some(id),
                _ => None,
            })
            .unwrap(),
        _ => unreachable!(),
    }
}

fn scheme_id_from_inverse(command: Command) -> SchemeId {
    match command {
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
