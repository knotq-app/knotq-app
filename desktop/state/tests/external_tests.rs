use chrono::NaiveDate;
use knotq_commands::Command;
use knotq_model::{AppSettings, Workspace};
use knotq_state::{AppState, ExternalModification, ExternalModificationQueue};

#[test]
fn external_modification_flows_through_queue() {
    let (queue, receiver) = ExternalModificationQueue::new();
    queue.push(MarkDirty).unwrap();

    let mut state = test_state();
    let modification = receiver.try_recv().unwrap();
    let command = modification.apply(&mut state).unwrap();

    assert!(state.is_dirty());
    assert!(command.is_none());
}

struct MarkDirty;

impl ExternalModification for MarkDirty {
    fn apply(&self, state: &mut AppState) -> anyhow::Result<Option<Command>> {
        state.mark_index_dirty();
        Ok(None)
    }
}

fn test_state() -> AppState {
    AppState::new(
        Workspace::new(),
        AppSettings::default(),
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(2025, 12, 1).unwrap(),
        false,
        Default::default(),
    )
}
