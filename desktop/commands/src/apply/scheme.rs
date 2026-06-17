use knotq_model::{FolderId, NodeRef, Scheme, SchemeId, SchemeSource, Workspace};

use crate::invariants::{is_valid_scheme_parent, validate_position, CommandError};
use crate::{ChangeSet, Command, CommandReceipt};

pub(crate) fn apply_scheme(
    workspace: &mut Workspace,
    cmd: Command,
) -> Result<CommandReceipt, CommandError> {
    match cmd {
        Command::CreateScheme {
            folder,
            name,
            color_index,
            position,
        } => create_scheme(workspace, folder, name, color_index, position),
        Command::RestoreScheme {
            folder,
            position,
            scheme,
        } => restore_scheme(workspace, folder, position, scheme),
        Command::RestoreDeletedScheme {
            position,
            scheme,
            origin,
        } => restore_deleted_scheme(workspace, position, scheme, origin),
        Command::RenameScheme { id, name } => rename_scheme(workspace, id, name),
        Command::SetSchemeColor { id, color_index } => set_scheme_color(workspace, id, color_index),
        Command::SetSchemeGsync { id, on } => set_scheme_gsync(workspace, id, on),
        Command::SetSchemeSource { id, source } => set_scheme_source(workspace, id, source),
        Command::DeleteScheme { id } => delete_scheme(workspace, id),
        Command::PermanentlyDeleteScheme { id } => permanently_delete_scheme(workspace, id),
        _ => unreachable!("non-scheme command dispatched to scheme handler"),
    }
}

fn create_scheme(
    workspace: &mut Workspace,
    folder: FolderId,
    name: String,
    color_index: u8,
    position: Option<usize>,
) -> Result<CommandReceipt, CommandError> {
    if !is_valid_scheme_parent(workspace, folder) {
        return Err(CommandError::BadFolderDepth);
    }
    let folder_obj = workspace
        .folders
        .get(&folder)
        .ok_or(CommandError::FolderMissing(folder))?;
    let pos = position.unwrap_or(folder_obj.children.len());
    validate_position(pos, folder_obj.children.len())?;
    let scheme = Scheme::new(name, color_index);
    let id = scheme.id;
    workspace.schemes.insert(id, scheme);
    workspace
        .folders
        .get_mut(&folder)
        .unwrap()
        .children
        .insert(pos, NodeRef::Scheme(id));
    Ok(CommandReceipt {
        inverse: Command::Batch(vec![
            Command::DeleteScheme { id },
            Command::PermanentlyDeleteScheme { id },
        ]),
        touched: ChangeSet::default().touched_folder(folder),
    })
}

fn restore_scheme(
    workspace: &mut Workspace,
    folder: FolderId,
    position: usize,
    mut scheme: Scheme,
) -> Result<CommandReceipt, CommandError> {
    if !is_valid_scheme_parent(workspace, folder) {
        return Err(CommandError::BadFolderDepth);
    }
    let folder_len = workspace
        .folders
        .get(&folder)
        .ok_or(CommandError::FolderMissing(folder))?
        .children
        .len();
    validate_position(position, folder_len)?;
    for item in &mut scheme.items {
        item.enforce_marker_constraints();
    }
    let id = scheme.id;
    workspace.unmark_scheme_deleted(id);
    // Detach from any current parent (e.g. an archived folder's retained subtree)
    // so restoring lands it only at the target folder, mirroring `restore_folder`.
    for folder in workspace.folders.values_mut() {
        folder
            .children
            .retain(|child| *child != NodeRef::Scheme(id));
    }
    workspace
        .folders
        .get_mut(&folder)
        .unwrap()
        .children
        .insert(position, NodeRef::Scheme(id));
    workspace.schemes.insert(id, scheme);
    Ok(CommandReceipt {
        inverse: Command::DeleteScheme { id },
        touched: ChangeSet::default().touched_folder(folder),
    })
}

fn restore_deleted_scheme(
    workspace: &mut Workspace,
    position: usize,
    mut scheme: Scheme,
    origin: Option<knotq_model::DeletedSchemeOrigin>,
) -> Result<CommandReceipt, CommandError> {
    validate_position(position, workspace.recently_deleted.len())?;
    for item in &mut scheme.items {
        item.enforce_marker_constraints();
    }
    let id = scheme.id;
    workspace.schemes.insert(id, scheme);
    workspace.mark_scheme_deleted_at(id, position);
    if let Some(origin) = origin {
        workspace.deleted_scheme_origins.insert(id, origin);
    } else {
        workspace.deleted_scheme_origins.remove(&id);
    }
    Ok(CommandReceipt {
        inverse: Command::PermanentlyDeleteScheme { id },
        touched: ChangeSet::default().touched_scheme(id),
    })
}

fn rename_scheme(
    workspace: &mut Workspace,
    id: SchemeId,
    name: String,
) -> Result<CommandReceipt, CommandError> {
    let scheme = workspace
        .schemes
        .get_mut(&id)
        .ok_or(CommandError::SchemeMissing(id))?;
    let prev = std::mem::replace(&mut scheme.name, name);
    Ok(CommandReceipt {
        inverse: Command::RenameScheme { id, name: prev },
        touched: ChangeSet::default().touched_scheme(id),
    })
}

fn set_scheme_color(
    workspace: &mut Workspace,
    id: SchemeId,
    color_index: u8,
) -> Result<CommandReceipt, CommandError> {
    let scheme = workspace
        .schemes
        .get_mut(&id)
        .ok_or(CommandError::SchemeMissing(id))?;
    let prev = scheme.color_index;
    scheme.color_index = color_index;
    Ok(CommandReceipt {
        inverse: Command::SetSchemeColor {
            id,
            color_index: prev,
        },
        touched: ChangeSet::default().touched_scheme(id),
    })
}

fn set_scheme_gsync(
    workspace: &mut Workspace,
    id: SchemeId,
    on: bool,
) -> Result<CommandReceipt, CommandError> {
    let scheme = workspace
        .schemes
        .get_mut(&id)
        .ok_or(CommandError::SchemeMissing(id))?;
    let prev = scheme.gsync;
    scheme.gsync = on;
    Ok(CommandReceipt {
        inverse: Command::SetSchemeGsync { id, on: prev },
        touched: ChangeSet::default().touched_scheme(id),
    })
}

fn set_scheme_source(
    workspace: &mut Workspace,
    id: SchemeId,
    source: SchemeSource,
) -> Result<CommandReceipt, CommandError> {
    let scheme = workspace
        .schemes
        .get_mut(&id)
        .ok_or(CommandError::SchemeMissing(id))?;
    let prev = std::mem::replace(&mut scheme.source, source);
    Ok(CommandReceipt {
        inverse: Command::SetSchemeSource { id, source: prev },
        touched: ChangeSet::default().touched_scheme(id),
    })
}

fn delete_scheme(workspace: &mut Workspace, id: SchemeId) -> Result<CommandReceipt, CommandError> {
    if !workspace.schemes.contains_key(&id) {
        return Err(CommandError::SchemeMissing(id));
    }
    let (parent_id, pos) = workspace
        .folders
        .iter()
        .find_map(|(fid, folder)| {
            folder
                .children
                .iter()
                .position(|child| *child == NodeRef::Scheme(id))
                .map(|pos| (*fid, pos))
        })
        .ok_or(CommandError::SchemeMissing(id))?;
    workspace
        .folders
        .get_mut(&parent_id)
        .unwrap()
        .children
        .remove(pos);
    for folder in workspace.folders.values_mut() {
        folder
            .children
            .retain(|child| *child != NodeRef::Scheme(id));
    }
    let removed = workspace.schemes.get(&id).cloned().unwrap();
    workspace.mark_scheme_deleted_from(id, parent_id, pos);
    Ok(CommandReceipt {
        inverse: Command::RestoreScheme {
            folder: parent_id,
            position: pos,
            scheme: removed,
        },
        touched: ChangeSet::default()
            .touched_folder(parent_id)
            .touched_scheme(id),
    })
}

fn permanently_delete_scheme(
    workspace: &mut Workspace,
    id: SchemeId,
) -> Result<CommandReceipt, CommandError> {
    let Some(trash_position) = workspace
        .recently_deleted
        .iter()
        .position(|deleted| *deleted == id)
    else {
        return Err(CommandError::SchemeMissing(id));
    };
    let removed = workspace
        .schemes
        .remove(&id)
        .ok_or(CommandError::SchemeMissing(id))?;
    workspace.recently_deleted.remove(trash_position);
    let origin = workspace.deleted_scheme_origins.remove(&id);
    Ok(CommandReceipt {
        inverse: Command::RestoreDeletedScheme {
            position: trash_position,
            scheme: removed,
            origin,
        },
        touched: ChangeSet::default().touched_scheme(id),
    })
}
