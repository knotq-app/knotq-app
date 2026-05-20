use chrono::{TimeZone, Utc};
use knotq_model::{CalendarRecurrence, Item};

use crate::make_minimal_workspace;

pub fn make_recurring_workspace() -> knotq_model::Workspace {
    let mut workspace = make_minimal_workspace();
    if let Some(scheme) = workspace.schemes.values_mut().next() {
        let mut item = Item::new("Standup")
            .with_start(Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap())
            .with_end(Utc.with_ymd_and_hms(2026, 1, 5, 9, 30, 0).unwrap());
        item.repeats = Some(CalendarRecurrence {
            rrules: vec!["FREQ=DAILY;COUNT=5".to_string()],
            ..CalendarRecurrence::default()
        });
        scheme.items = vec![item];
    }
    workspace
}
