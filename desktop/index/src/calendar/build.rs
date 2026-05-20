use knotq_model::{ItemKind, Workspace, DAILY_QUEUE_TITLE};
use knotq_rrule::OccurrenceExpander;

use super::{CalendarIndex, CalendarItemContext};

pub fn build_calendar_index(
    workspace: &Workspace,
    _expander: &dyn OccurrenceExpander,
) -> CalendarIndex {
    let mut items = Vec::new();
    for scheme in workspace.iter_schemes() {
        for item in &scheme.items {
            if matches!(
                item.kind(),
                ItemKind::Event | ItemKind::Reminder | ItemKind::Assignment
            ) {
                items.push(CalendarItemContext {
                    scheme_id: scheme.id,
                    item_id: item.id,
                    color_index: scheme.color_index,
                    scheme_name: if workspace.is_daily_queue_scheme(scheme.id) {
                        DAILY_QUEUE_TITLE.to_string()
                    } else {
                        scheme.name.clone()
                    },
                });
            }
        }
    }
    CalendarIndex { items }
}
