use thiserror::Error;

use knotq_model::{FolderId, NodeRef, SchemeId, Workspace, WorkspaceNodeNameError};

use crate::Command;

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("folder {0} not found")]
    FolderMissing(FolderId),
    #[error("scheme {0} not found")]
    SchemeMissing(SchemeId),
    #[error("item {0} not found in scheme {1}")]
    ItemMissing(knotq_model::ItemId, SchemeId),
    #[error("cannot delete the root folder")]
    DeleteRoot,
    #[error("cannot move a folder into itself or a descendant")]
    CycleMove,
    #[error("invalid folder depth")]
    BadFolderDepth,
    #[error("position {0} out of bounds")]
    BadPosition(usize),
    #[error("scheme {0} is read-only")]
    ReadOnlyScheme(SchemeId),
    #[error("invalid {kind} name {name:?}: {reason}")]
    InvalidNodeName {
        kind: &'static str,
        name: String,
        reason: WorkspaceNodeNameError,
    },
    #[error("folder name {name:?} already exists under folder {parent}")]
    DuplicateFolderName { parent: FolderId, name: String },
    #[error("scheme name {name:?} already exists under folder {parent}")]
    DuplicateSchemeName { parent: FolderId, name: String },
}

pub fn validate_position(position: usize, len: usize) -> Result<(), CommandError> {
    if position > len {
        Err(CommandError::BadPosition(position))
    } else {
        Ok(())
    }
}

pub fn is_descendant(workspace: &Workspace, candidate: FolderId, ancestor: FolderId) -> bool {
    let mut current = workspace.folders.get(&candidate).and_then(|f| f.parent);
    while let Some(parent) = current {
        if parent == ancestor {
            return true;
        }
        current = workspace.folders.get(&parent).and_then(|f| f.parent);
    }
    false
}

pub fn is_valid_scheme_parent(workspace: &Workspace, folder_id: FolderId) -> bool {
    workspace.folders.contains_key(&folder_id)
}

pub fn validate_depth_for_node(
    workspace: &Workspace,
    node: NodeRef,
    new_parent: FolderId,
) -> Result<(), CommandError> {
    match node {
        NodeRef::Folder(_) if !workspace.folders.contains_key(&new_parent) => {
            Err(CommandError::FolderMissing(new_parent))
        }
        NodeRef::Scheme(_) if !is_valid_scheme_parent(workspace, new_parent) => {
            Err(CommandError::FolderMissing(new_parent))
        }
        _ => Ok(()),
    }
}

pub fn validate_folder_name(name: &str) -> Result<(), CommandError> {
    let _ = name;
    Ok(())
}

pub fn validate_scheme_name(name: &str) -> Result<(), CommandError> {
    let _ = name;
    Ok(())
}

pub fn ensure_folder_name_available(
    workspace: &Workspace,
    parent: FolderId,
    name: &str,
    except: Option<FolderId>,
) -> Result<(), CommandError> {
    workspace
        .folders
        .get(&parent)
        .ok_or(CommandError::FolderMissing(parent))?;
    let _ = (name, except);
    Ok(())
}

pub fn ensure_scheme_name_available(
    workspace: &Workspace,
    parent: FolderId,
    name: &str,
    except: Option<SchemeId>,
) -> Result<(), CommandError> {
    workspace
        .folders
        .get(&parent)
        .ok_or(CommandError::FolderMissing(parent))?;
    let _ = (name, except);
    Ok(())
}

pub fn scheme_parent(workspace: &Workspace, id: SchemeId) -> Option<FolderId> {
    workspace.folders.iter().find_map(|(folder_id, folder)| {
        folder
            .children.contains(&NodeRef::Scheme(id))
            .then_some(*folder_id)
    })
}

pub fn validate_sibling_name_for_node(
    workspace: &Workspace,
    node: NodeRef,
    parent: FolderId,
) -> Result<(), CommandError> {
    match node {
        NodeRef::Folder(id) => {
            let name = workspace
                .folders
                .get(&id)
                .ok_or(CommandError::FolderMissing(id))?
                .name
                .clone();
            let _ = (name, parent);
            Ok(())
        }
        NodeRef::Scheme(id) => {
            let name = workspace
                .schemes
                .get(&id)
                .ok_or(CommandError::SchemeMissing(id))?
                .name
                .clone();
            let _ = (name, parent);
            Ok(())
        }
    }
}

pub fn ensure_command_allowed_for_user(
    workspace: &Workspace,
    cmd: &Command,
) -> Result<(), CommandError> {
    match cmd {
        Command::InsertItem { scheme, .. }
        | Command::UpdateItemText { scheme, .. }
        | Command::ReplaceItem { scheme, .. }
        | Command::SetItemIndent { scheme, .. }
        | Command::SetItemMarker { scheme, .. }
        | Command::SetItemDate { scheme, .. }
        | Command::SetItemRecurrence { scheme, .. }
        | Command::SetItemPriority { scheme, .. }
        | Command::SetOccurrenceNotificationOffset { scheme, .. }
        | Command::DeleteItem { scheme, .. }
        | Command::ReorderItem { scheme, .. }
        | Command::ToggleOccurrence { scheme, .. } => {
            if workspace
                .schemes
                .get(scheme)
                .ok_or(CommandError::SchemeMissing(*scheme))?
                .is_read_only()
            {
                return Err(CommandError::ReadOnlyScheme(*scheme));
            }
        }
        Command::SetSchemeColor { id, .. } => {
            workspace
                .schemes
                .get(id)
                .ok_or(CommandError::SchemeMissing(*id))?;
        }
        Command::SetSchemeGsync { id, .. } | Command::SetSchemeSource { id, .. } => {
            if workspace
                .schemes
                .get(id)
                .ok_or(CommandError::SchemeMissing(*id))?
                .is_read_only()
            {
                return Err(CommandError::ReadOnlyScheme(*id));
            }
        }
        Command::Batch(cmds) => {
            for cmd in cmds {
                ensure_command_allowed_for_user(workspace, cmd)?;
            }
        }
        _ => {}
    }
    Ok(())
}
