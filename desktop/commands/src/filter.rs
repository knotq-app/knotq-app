use crate::Command;
use knotq_model::{OccurrenceId, Workspace};

pub fn filter_recurring_occurrence_toggles(
    command: Command,
    workspace: &Workspace,
) -> Option<Command> {
    match command {
        Command::ToggleOccurrence {
            scheme,
            item,
            occurrence,
        } => {
            let item_ref = workspace.scheme(scheme)?.item(item)?;
            if item_ref.repeats.is_some() {
                matches!(occurrence, OccurrenceId::Recurring { .. }).then_some(
                    Command::ToggleOccurrence {
                        scheme,
                        item,
                        occurrence,
                    },
                )
            } else {
                Some(Command::ToggleOccurrence {
                    scheme,
                    item,
                    occurrence,
                })
            }
        }
        Command::Batch(commands) => {
            let filtered = commands
                .into_iter()
                .filter_map(|command| filter_recurring_occurrence_toggles(command, workspace))
                .collect::<Vec<_>>();
            (!filtered.is_empty()).then_some(Command::Batch(filtered))
        }
        other => Some(other),
    }
}
