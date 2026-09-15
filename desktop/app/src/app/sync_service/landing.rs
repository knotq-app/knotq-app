//! Landing a finished sync run on the live state — the decisions the sync task
//! makes on the UI thread once `sync_snapshot` returns, kept out of the GPUI
//! closure so the production-path fuzzer runs exactly the same steps.

use std::collections::HashMap;
use std::sync::Arc;

use knotq_model::{DocumentId, Workspace};
use knotq_state::AppState;
use knotq_sync::PushedDocument;

/// Drop the pending edits a run pushed, and any the run discarded because their
/// document is no longer bound (see `drop_unbound_pending_crdt_edits`).
///
/// `local_edit_watermark` is the store's sequence when the run took its
/// snapshot: an edit made after it was not in the run, whatever its sequence.
pub(super) fn clear_pushed_edits(
    state: &mut AppState,
    pushed: &[PushedDocument],
    local_edit_watermark: u64,
) {
    for pushed in pushed {
        state.clear_pushed_crdt_edits(
            pushed.document,
            pushed.through_local_sequence,
            local_edit_watermark,
        );
    }
    state.drop_unbound_pending_crdt_edits();
}

/// The line edits still queued before a landing clears what the run pushed, so
/// a line another device moved to another scheme can keep them (see
/// [`reassert_local_item_edits`]).
pub(super) fn capture_local_item_edits(state: &AppState) -> knotq_state::LocalItemEdits {
    state.capture_local_item_edits()
}

/// After landing: re-apply this device's line edits to lines another device
/// moved to a different scheme, whose moved copy lost them. Returns whether any
/// line changed.
pub(super) fn reassert_local_item_edits(
    state: &mut AppState,
    captured: knotq_state::LocalItemEdits,
) -> bool {
    state.reassert_local_item_edits(captured) > 0
}

/// Whether a run's result has to be landed on the live workspace at all.
pub(super) fn run_changed_workspace(
    remote_updates_applied: usize,
    local_workspace_changed: bool,
) -> bool {
    remote_updates_applied > 0 || local_workspace_changed
}

/// Adopt a run's merged workspace. Edits applied while the run was in flight
/// are not in its result, so the result is merged into the live documents and
/// they survive; with none in flight the replace is equivalent and adopts the
/// run's canonical state wholesale. Returns whether the workspace visibly changed
/// (the merge path is always treated as changed — the user is mid-edit).
pub(super) fn adopt_sync_workspace(
    state: &mut AppState,
    workspace: Workspace,
    crdt_states: HashMap<DocumentId, Arc<[u8]>>,
    local_edit_watermark: u64,
) -> bool {
    let merged = state.has_local_edits_since(local_edit_watermark)
        && state.merge_workspace_from_sync(&workspace, &crdt_states);
    if merged {
        true
    } else {
        state.replace_workspace_from_sync(workspace, crdt_states)
    }
}
