use thiserror::Error;

use knotq_model::{
    FolderId, NodeRef, SchemeId, Workspace, WorkspaceNodeNameError, WorkspaceNodeNameKind,
};

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
    #[error("folders can only be created at the workspace root")]
    BadFolderDepth,
    #[error("position {0} out of bounds")]
    BadPosition(usize),
    #[error("occurrence {0} out of bounds")]
    BadOccurrence(usize),
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
    folder_id == workspace.root
        || workspace
            .folders
            .get(&folder_id)
            .is_some_and(|folder| folder.parent == Some(workspace.root))
}

pub fn validate_depth_for_node(
    workspace: &Workspace,
    node: NodeRef,
    new_parent: FolderId,
) -> Result<(), CommandError> {
    match node {
        NodeRef::Folder(_) if new_parent != workspace.root => Err(CommandError::BadFolderDepth),
        NodeRef::Scheme(_) if !is_valid_scheme_parent(workspace, new_parent) => {
            Err(CommandError::BadFolderDepth)
        }
        _ => Ok(()),
    }
}

pub fn validate_folder_name(name: &str) -> Result<(), CommandError> {
    validate_node_name(name, WorkspaceNodeNameKind::Folder)
}

pub fn validate_scheme_name(name: &str) -> Result<(), CommandError> {
    validate_node_name(name, WorkspaceNodeNameKind::Scheme)
}

pub fn ensure_folder_name_available(
    workspace: &Workspace,
    parent: FolderId,
    name: &str,
    except: Option<FolderId>,
) -> Result<(), CommandError> {
    let folder = workspace
        .folders
        .get(&parent)
        .ok_or(CommandError::FolderMissing(parent))?;
    let exists = folder.children.iter().any(|child| {
        let NodeRef::Folder(id) = child else {
            return false;
        };
        if Some(*id) == except {
            return false;
        }
        workspace
            .folders
            .get(id)
            .is_some_and(|folder| names_match(&folder.name, name))
    });
    if exists {
        Err(CommandError::DuplicateFolderName {
            parent,
            name: name.to_string(),
        })
    } else {
        Ok(())
    }
}

pub fn ensure_scheme_name_available(
    workspace: &Workspace,
    parent: FolderId,
    name: &str,
    except: Option<SchemeId>,
) -> Result<(), CommandError> {
    let folder = workspace
        .folders
        .get(&parent)
        .ok_or(CommandError::FolderMissing(parent))?;
    let exists = folder.children.iter().any(|child| {
        let NodeRef::Scheme(id) = child else {
            return false;
        };
        if Some(*id) == except {
            return false;
        }
        workspace
            .schemes
            .get(id)
            .is_some_and(|scheme| names_match(&scheme.name, name))
    });
    if exists {
        Err(CommandError::DuplicateSchemeName {
            parent,
            name: name.to_string(),
        })
    } else {
        Ok(())
    }
}

pub fn scheme_parent(workspace: &Workspace, id: SchemeId) -> Option<FolderId> {
    workspace.folders.iter().find_map(|(folder_id, folder)| {
        folder
            .children
            .iter()
            .any(|child| *child == NodeRef::Scheme(id))
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
            validate_folder_name(&name)?;
            ensure_folder_name_available(workspace, parent, &name, Some(id))
        }
        NodeRef::Scheme(id) => {
            let name = workspace
                .schemes
                .get(&id)
                .ok_or(CommandError::SchemeMissing(id))?
                .name
                .clone();
            validate_scheme_name(&name)?;
            ensure_scheme_name_available(workspace, parent, &name, Some(id))
        }
    }
}

pub fn enforce_marker_constraints(item: &mut knotq_model::Item) {
    item.enforce_marker_constraints();
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
        Command::SetSchemeColor { id, .. }
        | Command::SetSchemeGsync { id, .. }
        | Command::SetSchemeSource { id, .. } => {
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

fn validate_node_name(name: &str, kind: WorkspaceNodeNameKind) -> Result<(), CommandError> {
    knotq_model::validate_workspace_node_name(name, kind).map_err(|reason| {
        CommandError::InvalidNodeName {
            kind: match kind {
                WorkspaceNodeNameKind::Folder => "folder",
                WorkspaceNodeNameKind::Scheme => "scheme",
            },
            name: name.to_string(),
            reason,
        }
    })
}

fn names_match(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}
