use knotq_model::{Folder, FolderId, NodeRef, Workspace};

use crate::invariants::{
    ensure_folder_name_available, validate_folder_name, validate_position, CommandError,
};
use crate::{ChangeSet, Command, CommandReceipt};

pub(crate) fn apply_folder(
    workspace: &mut Workspace,
    cmd: Command,
) -> Result<CommandReceipt, CommandError> {
    match cmd {
        Command::CreateFolder {
            parent,
            name,
            position,
        } => create_folder(workspace, parent, name, position),
        Command::RestoreFolder {
            parent,
            position,
            folder,
        } => restore_folder(workspace, parent, position, folder),
        Command::RenameFolder { id, name } => rename_folder(workspace, id, name),
        Command::SetFolderExpanded { id, expanded } => set_folder_expanded(workspace, id, expanded),
        Command::DeleteFolder { id } => delete_folder(workspace, id),
        _ => unreachable!("non-folder command dispatched to folder handler"),
    }
}

fn create_folder(
    workspace: &mut Workspace,
    parent: FolderId,
    name: String,
    position: Option<usize>,
) -> Result<CommandReceipt, CommandError> {
    if parent != workspace.root {
        return Err(CommandError::BadFolderDepth);
    }
    validate_folder_name(&name)?;
    ensure_folder_name_available(workspace, parent, &name, None)?;
    let parent_folder = workspace
        .folders
        .get(&parent)
        .ok_or(CommandError::FolderMissing(parent))?;
    let pos = position.unwrap_or(parent_folder.children.len());
    validate_position(pos, parent_folder.children.len())?;
    let new = Folder {
        id: FolderId::new(),
        name,
        parent: Some(parent),
        children: Vec::new(),
        expanded: true,
    };
    let new_id = new.id;
    workspace.folders.insert(new_id, new);
    workspace
        .folders
        .get_mut(&parent)
        .unwrap()
        .children
        .insert(pos, NodeRef::Folder(new_id));
    Ok(CommandReceipt {
        inverse: Command::DeleteFolder { id: new_id },
        touched: ChangeSet::default().touched_folder(parent),
    })
}

fn restore_folder(
    workspace: &mut Workspace,
    parent: FolderId,
    position: usize,
    folder: Folder,
) -> Result<CommandReceipt, CommandError> {
    if parent != workspace.root {
        return Err(CommandError::BadFolderDepth);
    }
    validate_folder_name(&folder.name)?;
    ensure_folder_name_available(workspace, parent, &folder.name, Some(folder.id))?;
    let parent_len = workspace
        .folders
        .get(&parent)
        .ok_or(CommandError::FolderMissing(parent))?
        .children
        .len();
    validate_position(position, parent_len)?;
    let id = folder.id;
    for child in &folder.children {
        match child {
            NodeRef::Folder(id) if !workspace.folders.contains_key(id) => {
                return Err(CommandError::FolderMissing(*id));
            }
            NodeRef::Folder(_) => return Err(CommandError::BadFolderDepth),
            NodeRef::Scheme(id) if !workspace.schemes.contains_key(id) => {
                return Err(CommandError::SchemeMissing(*id));
            }
            NodeRef::Scheme(_) => {}
        }
    }
    workspace
        .folders
        .get_mut(&parent)
        .unwrap()
        .children
        .insert(position, NodeRef::Folder(id));
    workspace.folders.insert(id, folder);
    Ok(CommandReceipt {
        inverse: Command::DeleteFolder { id },
        touched: ChangeSet::default().touched_folder(parent),
    })
}

fn rename_folder(
    workspace: &mut Workspace,
    id: FolderId,
    name: String,
) -> Result<CommandReceipt, CommandError> {
    validate_folder_name(&name)?;
    let parent = workspace
        .folders
        .get(&id)
        .ok_or(CommandError::FolderMissing(id))?
        .parent;
    if let Some(parent) = parent {
        ensure_folder_name_available(workspace, parent, &name, Some(id))?;
    }
    let folder = workspace
        .folders
        .get_mut(&id)
        .ok_or(CommandError::FolderMissing(id))?;
    let prev = std::mem::replace(&mut folder.name, name);
    Ok(CommandReceipt {
        inverse: Command::RenameFolder { id, name: prev },
        touched: ChangeSet::default().touched_folder(id),
    })
}

fn set_folder_expanded(
    workspace: &mut Workspace,
    id: FolderId,
    expanded: bool,
) -> Result<CommandReceipt, CommandError> {
    let folder = workspace
        .folders
        .get_mut(&id)
        .ok_or(CommandError::FolderMissing(id))?;
    let prev = folder.expanded;
    folder.expanded = expanded;
    Ok(CommandReceipt {
        inverse: Command::SetFolderExpanded { id, expanded: prev },
        touched: ChangeSet::default().touched_folder(id),
    })
}

fn delete_folder(workspace: &mut Workspace, id: FolderId) -> Result<CommandReceipt, CommandError> {
    if id == workspace.root {
        return Err(CommandError::DeleteRoot);
    }
    let folder = workspace
        .folders
        .get(&id)
        .ok_or(CommandError::FolderMissing(id))?;
    let parent = folder.parent.ok_or(CommandError::DeleteRoot)?;
    let parent_folder = workspace
        .folders
        .get_mut(&parent)
        .ok_or(CommandError::FolderMissing(parent))?;
    let pos = parent_folder
        .children
        .iter()
        .position(|child| *child == NodeRef::Folder(id))
        .ok_or(CommandError::FolderMissing(id))?;
    parent_folder.children.remove(pos);
    let removed = workspace.folders.remove(&id).unwrap();
    let mut removed_schemes = Vec::new();
    for (position, child) in removed.children.iter().enumerate() {
        if let NodeRef::Scheme(scheme_id) = child {
            if let Some(scheme) = workspace.schemes.get(scheme_id).cloned() {
                workspace.mark_scheme_deleted_from(*scheme_id, id, position);
                removed_schemes.push((position, scheme));
            }
        }
    }
    let mut folder_shell = removed;
    folder_shell
        .children
        .retain(|child| !matches!(child, NodeRef::Scheme(_)));
    let mut inverse = Vec::with_capacity(1 + removed_schemes.len());
    inverse.push(Command::RestoreFolder {
        parent,
        position: pos,
        folder: folder_shell,
    });
    for (position, scheme) in &removed_schemes {
        inverse.push(Command::RestoreScheme {
            folder: id,
            position: *position,
            scheme: scheme.clone(),
        });
    }
    Ok(CommandReceipt {
        inverse: Command::Batch(inverse),
        touched: removed_schemes.iter().fold(
            ChangeSet::default().touched_folder(parent),
            |changes, (_, scheme)| changes.touched_scheme(scheme.id),
        ),
    })
}
