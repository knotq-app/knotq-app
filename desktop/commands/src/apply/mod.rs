mod batch;
mod folder;
mod item;
mod scheme;

use knotq_model::{FolderId, NodeRef, Workspace};

use crate::invariants::{
    ensure_command_allowed_for_user, is_descendant, validate_depth_for_node,
    validate_sibling_name_for_node, CommandError,
};
use crate::{ChangeSet, Command, CommandOrigin, CommandReceipt};

pub fn apply(
    workspace: &mut Workspace,
    cmd: Command,
    origin: CommandOrigin,
) -> Result<CommandReceipt, CommandError> {
    if origin == CommandOrigin::User {
        ensure_command_allowed_for_user(workspace, &cmd)?;
    }
    dispatch(workspace, cmd, origin)
}

pub(crate) fn dispatch(
    workspace: &mut Workspace,
    cmd: Command,
    origin: CommandOrigin,
) -> Result<CommandReceipt, CommandError> {
    match cmd {
        Command::CreateFolder { .. }
        | Command::RestoreFolder { .. }
        | Command::RenameFolder { .. }
        | Command::SetFolderExpanded { .. }
        | Command::DeleteFolder { .. } => folder::apply_folder(workspace, cmd),
        Command::CreateScheme { .. }
        | Command::RestoreScheme { .. }
        | Command::RestoreDeletedScheme { .. }
        | Command::RenameScheme { .. }
        | Command::SetSchemeColor { .. }
        | Command::SetSchemeGsync { .. }
        | Command::SetSchemeSource { .. }
        | Command::DeleteScheme { .. }
        | Command::PermanentlyDeleteScheme { .. } => scheme::apply_scheme(workspace, cmd),
        Command::MoveNode {
            node,
            new_parent,
            position,
        } => move_node(workspace, node, new_parent, position),
        Command::InsertItem { .. }
        | Command::UpdateItemText { .. }
        | Command::ReplaceItem { .. }
        | Command::SetItemIndent { .. }
        | Command::SetItemMarker { .. }
        | Command::SetItemDate { .. }
        | Command::SetItemRecurrence { .. }
        | Command::SetItemPriority { .. }
        | Command::SetOccurrenceNotificationOffset { .. }
        | Command::ToggleOccurrence { .. }
        | Command::DeleteItem { .. }
        | Command::ReorderItem { .. } => item::apply_item(workspace, cmd),
        Command::Batch(cmds) => batch::apply_batch(workspace, cmds, origin),
    }
}

pub fn move_node(
    workspace: &mut Workspace,
    node: NodeRef,
    new_parent: FolderId,
    position: usize,
) -> Result<CommandReceipt, CommandError> {
    validate_depth_for_node(workspace, node, new_parent)?;
    if let NodeRef::Folder(fid) = node {
        if fid == new_parent || is_descendant(workspace, new_parent, fid) {
            return Err(CommandError::CycleMove);
        }
    }

    let (old_parent, old_pos) = workspace
        .folders
        .iter()
        .find_map(|(fid, f)| {
            f.children
                .iter()
                .position(|c| *c == node)
                .map(|pos| (*fid, pos))
        })
        .ok_or(match node {
            NodeRef::Folder(id) => CommandError::FolderMissing(id),
            NodeRef::Scheme(id) => CommandError::SchemeMissing(id),
        })?;

    validate_sibling_name_for_node(workspace, node, new_parent)?;

    workspace
        .folders
        .get_mut(&old_parent)
        .unwrap()
        .children
        .remove(old_pos);

    let adjusted_pos = position;

    let new_parent_obj = workspace
        .folders
        .get_mut(&new_parent)
        .ok_or(CommandError::FolderMissing(new_parent))?;
    if adjusted_pos > new_parent_obj.children.len() {
        let parent = workspace.folders.get_mut(&old_parent).unwrap();
        parent.children.insert(old_pos, node);
        return Err(CommandError::BadPosition(adjusted_pos));
    }
    new_parent_obj.children.insert(adjusted_pos, node);

    if let NodeRef::Folder(fid) = node {
        if let Some(folder) = workspace.folders.get_mut(&fid) {
            folder.parent = Some(new_parent);
        }
    }

    let mut touched = ChangeSet::default();
    touched.folders.push(old_parent);
    touched.folders.push(new_parent);
    Ok(CommandReceipt {
        inverse: Command::MoveNode {
            node,
            new_parent: old_parent,
            position: old_pos,
        },
        touched,
    })
}
