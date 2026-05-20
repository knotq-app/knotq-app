use knotq_model::Workspace;

use crate::apply::dispatch;
use crate::invariants::CommandError;
use crate::{ChangeSet, Command, CommandOrigin, CommandReceipt};

pub(crate) fn apply_batch(
    workspace: &mut Workspace,
    cmds: Vec<Command>,
    origin: CommandOrigin,
) -> Result<CommandReceipt, CommandError> {
    let mut inverses = Vec::with_capacity(cmds.len());
    let mut touched = ChangeSet::default();
    for cmd in cmds {
        let receipt = dispatch(workspace, cmd, origin)?;
        inverses.push(receipt.inverse);
        touched.merge(receipt.touched);
    }
    inverses.reverse();
    Ok(CommandReceipt {
        inverse: Command::Batch(inverses),
        touched,
    })
}
