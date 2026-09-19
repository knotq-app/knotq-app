//! Landing a finished sync run on the live state — the decisions the sync task
//! makes on the UI thread once `sync_snapshot` returns, kept out of the GPUI
//! closure so the production-path fuzzer runs exactly the same steps.

use std::collections::{HashMap, HashSet};
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
    _local_edit_watermark: u64,
) {
    for pushed in pushed {
        state.clear_pushed_crdt_edits_exact(pushed.document, &pushed.sent_edits);
    }
    state.drop_unbound_pending_crdt_edits();
}

/// The line edits still queued before a landing clears what the run pushed, so
/// a line another device moved to another scheme can keep them (see
/// [`reassert_local_item_edits`]).
pub(super) fn capture_local_item_edits(
    state: &AppState,
    queued: &[knotq_sync::QueuedItemFields],
    baseline: &Workspace,
) -> knotq_state::LocalItemEdits {
    state.capture_local_item_edits(queued, baseline)
}

pub(super) fn capture_local_scheme_edits(state: &AppState) -> knotq_state::LocalSchemeEdits {
    state.capture_local_scheme_edits()
}

pub(super) fn capture_local_folder_edits(
    state: &AppState,
    incoming: &Workspace,
) -> knotq_state::LocalFolderEdits {
    state.capture_local_folder_edits(incoming)
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

pub(super) fn reassert_recent_moved_item_edits(
    state: &mut AppState,
    skip_items: &HashSet<knotq_model::ItemId>,
) -> bool {
    state.reassert_recent_moved_item_edits(skip_items) > 0
}

pub(super) fn reconcile_item_placements(state: &mut AppState) -> bool {
    state.reconcile_item_placements()
}

pub(super) fn reassert_local_scheme_edits(
    state: &mut AppState,
    captured: knotq_state::LocalSchemeEdits,
) -> bool {
    state.reassert_local_scheme_edits(captured) > 0
}

pub(super) fn reassert_local_folder_edits(
    state: &mut AppState,
    captured: knotq_state::LocalFolderEdits,
) -> bool {
    state.reassert_local_folder_edits(captured) > 0
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
    squash_applied: bool,
) -> bool {
    let has_local_edits = state.has_local_edits_since(local_edit_watermark);
    let merged = has_local_edits && state.merge_workspace_from_sync(&workspace, &crdt_states);
    if merged {
        true
    } else if !has_local_edits {
        // The sync result is a complete, already-merged snapshot. When no
        // local command landed while the request was in flight, adopting it
        // wholesale is the canonical boundary: incrementally unioning it with
        // a stale local CRDT can preserve an operation the server never
        // accepted (for example a reasserted move's position/text), leaving
        // this device different even though its pending queue is empty. The
        // incremental path is reserved for the only case that needs it: a
        // local edit that must be replayed over the returned snapshot.
        state.replace_workspace_from_sync_result(workspace, crdt_states)
    } else if squash_applied {
        state.replace_workspace_from_squash(workspace, crdt_states)
    } else {
        state.replace_workspace_from_sync(workspace, crdt_states)
    }
}
