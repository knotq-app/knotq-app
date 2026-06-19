use knotq_commands::Command;
use knotq_state::{AppState, ExternalModification, ExternalModificationQueue};

mod support;

use support::test_state;

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
