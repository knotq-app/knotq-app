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
        match dispatch(workspace, cmd, origin) {
            Ok(receipt) => {
                inverses.push(receipt.inverse);
                touched.merge(receipt.touched);
            }
            Err(err) => {
                // A batch is all-or-nothing: if any sub-command fails, undo the
                // sub-commands already applied (in reverse) so the workspace is
                // left exactly as it was before the batch. Without this, a
                // partially applied batch — e.g. a cross-scheme undo whose
                // second leg no longer applies — would corrupt the workspace.
                for inverse in inverses.into_iter().rev() {
                    let _ = dispatch(workspace, inverse, origin);
                }
                return Err(err);
            }
        }
    }
    inverses.reverse();
    Ok(CommandReceipt {
        inverse: Command::Batch(inverses),
        touched,
    })
}
