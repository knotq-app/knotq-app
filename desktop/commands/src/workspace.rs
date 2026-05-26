use knotq_model::{FolderId, NodeRef, Workspace};

use crate::{Command, CommandError, CommandOrigin, CommandReceipt};

pub trait WorkspaceCommandExt {
    fn apply(&mut self, cmd: Command) -> Result<CommandReceipt, CommandError>;
    fn apply_with_origin(
        &mut self,
        cmd: Command,
        origin: CommandOrigin,
    ) -> Result<CommandReceipt, CommandError>;
    fn ensure_command_allowed_for_user(&self, cmd: &Command) -> Result<(), CommandError>;
    fn move_node(
        &mut self,
        node: NodeRef,
        new_parent: FolderId,
        position: usize,
    ) -> Result<CommandReceipt, CommandError>;
}

impl WorkspaceCommandExt for Workspace {
    fn apply(&mut self, cmd: Command) -> Result<CommandReceipt, CommandError> {
        self.apply_with_origin(cmd, CommandOrigin::User)
    }

    fn apply_with_origin(
        &mut self,
        cmd: Command,
        origin: CommandOrigin,
    ) -> Result<CommandReceipt, CommandError> {
        crate::apply::apply(self, cmd, origin)
    }

    fn ensure_command_allowed_for_user(&self, cmd: &Command) -> Result<(), CommandError> {
        crate::invariants::ensure_command_allowed_for_user(self, cmd)
    }

    fn move_node(
        &mut self,
        node: NodeRef,
        new_parent: FolderId,
        position: usize,
    ) -> Result<CommandReceipt, CommandError> {
        crate::apply::move_node(self, node, new_parent, position)
    }
}
