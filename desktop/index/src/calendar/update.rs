use knotq_model::Workspace;
use knotq_rrule::OccurrenceExpander;

use crate::IndexChangeSet;

use super::{build_calendar_index, CalendarIndex};

pub fn update_calendar_index(
    index: &mut CalendarIndex,
    changeset: &IndexChangeSet,
    workspace: &Workspace,
    expander: &dyn OccurrenceExpander,
) {
    if changeset.folders.is_empty() && changeset.schemes.is_empty() {
        return;
    }
    *index = build_calendar_index(workspace, expander);
}
