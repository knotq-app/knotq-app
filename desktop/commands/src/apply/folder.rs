use knotq_model::{DeletedFolderOrigin, Folder, FolderId, NodeRef, Workspace};

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
        Command::RestoreDeletedFolder {
            folder,
            position,
            folders,
            schemes,
            origin,
        } => restore_deleted_folder(workspace, folder, position, folders, schemes, origin),
        Command::RenameFolder { id, name } => rename_folder(workspace, id, name),
        Command::SetFolderExpanded { id, expanded } => set_folder_expanded(workspace, id, expanded),
        Command::DeleteFolder { id } => delete_folder(workspace, id),
        Command::PermanentlyDeleteFolder { id } => permanently_delete_folder(workspace, id),
        _ => unreachable!("non-folder command dispatched to folder handler"),
    }
}

fn create_folder(
    workspace: &mut Workspace,
    parent: FolderId,
    name: String,
    position: Option<usize>,
) -> Result<CommandReceipt, CommandError> {
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
        inverse: Command::Batch(vec![
            Command::DeleteFolder { id: new_id },
            Command::PermanentlyDeleteFolder { id: new_id },
        ]),
        touched: ChangeSet::default().touched_folder(parent),
    })
}

fn restore_folder(
    workspace: &mut Workspace,
    parent: FolderId,
    position: usize,
    folder: Folder,
) -> Result<CommandReceipt, CommandError> {
    validate_folder_name(&folder.name)?;
    ensure_folder_name_available(workspace, parent, &folder.name, Some(folder.id))?;
    workspace
        .folders
        .get(&parent)
        .ok_or(CommandError::FolderMissing(parent))?;
    let id = folder.id;
    let existing = workspace.folders.get(&id).cloned().unwrap_or(folder);
    validate_folder_children(workspace, &existing)?;

    for folder in workspace.folders.values_mut() {
        folder
            .children
            .retain(|child| *child != NodeRef::Folder(id));
    }
    let parent_len = workspace
        .folders
        .get(&parent)
        .ok_or(CommandError::FolderMissing(parent))?
        .children
        .len();
    validate_position(position, parent_len)?;

    workspace.folders.insert(id, existing);
    if let Some(restored) = workspace.folders.get_mut(&id) {
        restored.parent = Some(parent);
    }
    let restored_scheme_ids = workspace.subtree_scheme_ids(id);
    workspace.unmark_folder_deleted(id);
    workspace
        .folders
        .get_mut(&parent)
        .unwrap()
        .children
        .insert(position, NodeRef::Folder(id));
    let touched = restored_scheme_ids.into_iter().fold(
        ChangeSet::default()
            .touched_folder(parent)
            .touched_folder(id),
        |changes, scheme| changes.touched_scheme(scheme),
    );
    Ok(CommandReceipt {
        inverse: Command::DeleteFolder { id },
        touched,
    })
}

fn restore_deleted_folder(
    workspace: &mut Workspace,
    folder_id: FolderId,
    position: usize,
    folders: Vec<Folder>,
    schemes: Vec<knotq_model::Scheme>,
    origin: Option<DeletedFolderOrigin>,
) -> Result<CommandReceipt, CommandError> {
    if !folders.iter().any(|folder| folder.id == folder_id) {
        return Err(CommandError::FolderMissing(folder_id));
    }

    for mut scheme in schemes {
        for item in &mut scheme.items {
            item.enforce_marker_constraints();
        }
        workspace.schemes.insert(scheme.id, scheme);
    }
    for folder in &folders {
        validate_folder_name(&folder.name)?;
    }
    for folder in folders {
        workspace.folders.insert(folder.id, folder);
    }
    let restored_folder_ids = workspace.subtree_folder_ids(folder_id);
    for folder_id in &restored_folder_ids {
        let Some(folder) = workspace.folders.get(folder_id) else {
            return Err(CommandError::FolderMissing(*folder_id));
        };
        validate_folder_children(workspace, folder)?;
    }
    for folder in workspace.folders.values_mut() {
        folder
            .children
            .retain(|child| *child != NodeRef::Folder(folder_id));
    }

    let origin = origin.unwrap_or_else(|| DeletedFolderOrigin {
        parent: workspace
            .folders
            .get(&folder_id)
            .and_then(|folder| folder.parent)
            .unwrap_or(workspace.root),
        position: 0,
    });
    workspace.mark_folder_deleted_at(folder_id, position, origin);
    let touched = workspace.subtree_scheme_ids(folder_id).into_iter().fold(
        ChangeSet::default().touched_folder(folder_id),
        |changes, scheme| changes.touched_scheme(scheme),
    );
    Ok(CommandReceipt {
        inverse: Command::PermanentlyDeleteFolder { id: folder_id },
        touched,
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
    let archived_scheme_ids = workspace.subtree_scheme_ids(id);
    let folder_snapshot = workspace.folders.get(&id).cloned().unwrap();
    workspace.mark_folder_deleted_from(id, parent, pos);
    let touched = archived_scheme_ids.into_iter().fold(
        ChangeSet::default()
            .touched_folder(parent)
            .touched_folder(id),
        |changes, scheme| changes.touched_scheme(scheme),
    );
    Ok(CommandReceipt {
        inverse: Command::RestoreFolder {
            parent,
            position: pos,
            folder: folder_snapshot,
        },
        touched,
    })
}

fn permanently_delete_folder(
    workspace: &mut Workspace,
    id: FolderId,
) -> Result<CommandReceipt, CommandError> {
    let Some(trash_position) = workspace
        .recently_deleted_folders
        .iter()
        .position(|deleted| *deleted == id)
    else {
        return Err(CommandError::FolderMissing(id));
    };
    if !workspace.folders.contains_key(&id) {
        return Err(CommandError::FolderMissing(id));
    }

    let folder_ids = ordered_subtree_folder_ids(workspace, id);
    let scheme_ids = ordered_subtree_scheme_ids(workspace, id);
    let folders = folder_ids
        .iter()
        .filter_map(|folder_id| workspace.folders.get(folder_id).cloned())
        .collect::<Vec<_>>();
    let schemes = scheme_ids
        .iter()
        .filter_map(|scheme_id| workspace.schemes.get(scheme_id).cloned())
        .collect::<Vec<_>>();
    let origin = workspace.deleted_folder_origins.remove(&id);
    workspace.recently_deleted_folders.remove(trash_position);

    for scheme_id in &scheme_ids {
        workspace.remove_scheme_from_archive(*scheme_id);
        workspace.schemes.remove(scheme_id);
    }
    for folder_id in &folder_ids {
        workspace.folders.remove(folder_id);
        workspace.folder_sync.remove(folder_id);
    }
    for folder in workspace.folders.values_mut() {
        folder.children.retain(|child| match child {
            NodeRef::Folder(folder_id) => !folder_ids.contains(folder_id),
            NodeRef::Scheme(scheme_id) => !scheme_ids.contains(scheme_id),
        });
    }

    let touched = scheme_ids.iter().copied().fold(
        folder_ids
            .iter()
            .copied()
            .fold(ChangeSet::default(), |changes, folder| {
                changes.touched_folder(folder)
            }),
        |changes, scheme| changes.touched_scheme(scheme),
    );
    Ok(CommandReceipt {
        inverse: Command::RestoreDeletedFolder {
            folder: id,
            position: trash_position,
            folders,
            schemes,
            origin,
        },
        touched,
    })
}

fn validate_folder_children(workspace: &Workspace, folder: &Folder) -> Result<(), CommandError> {
    for child in &folder.children {
        match child {
            NodeRef::Folder(id) if !workspace.folders.contains_key(id) && *id != folder.id => {
                return Err(CommandError::FolderMissing(*id));
            }
            NodeRef::Folder(_) => {}
            NodeRef::Scheme(id) if !workspace.schemes.contains_key(id) => {
                return Err(CommandError::SchemeMissing(*id));
            }
            NodeRef::Scheme(_) => {}
        }
    }
    Ok(())
}

fn ordered_subtree_folder_ids(workspace: &Workspace, root: FolderId) -> Vec<FolderId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(folder_id) = stack.pop() {
        if out.contains(&folder_id) {
            continue;
        }
        out.push(folder_id);
        if let Some(folder) = workspace.folders.get(&folder_id) {
            for child in folder.children.iter().rev() {
                if let NodeRef::Folder(id) = child {
                    stack.push(*id);
                }
            }
        }
    }
    out
}

fn ordered_subtree_scheme_ids(workspace: &Workspace, root: FolderId) -> Vec<knotq_model::SchemeId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(folder_id) = stack.pop() {
        let Some(folder) = workspace.folders.get(&folder_id) else {
            continue;
        };
        for child in &folder.children {
            match child {
                NodeRef::Scheme(id) if !out.contains(id) => out.push(*id),
                NodeRef::Scheme(_) => {}
                NodeRef::Folder(id) => stack.push(*id),
            }
        }
    }
    out
}
