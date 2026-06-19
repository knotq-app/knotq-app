use std::collections::HashMap;

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{AppSettings, DocumentId, Item, NodeRef, ReplicaId, Scheme, SchemeId, Workspace};
use knotq_state::AppState;
use knotq_sync::{WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments};

mod support;

use support::date;

fn app_state_with_scheme(name: &str) -> (AppState, SchemeId) {
    let mut workspace = Workspace::new();
    let scheme = Scheme::new(name, 0);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    let settings = AppSettings::default();
    // Seed populated CRDT documents, as a synced device restoring from disk
    // would have — both sides then diff against a shared base instead of
    // bootstrapping the whole index concurrently.
    workspace.ensure_sync_metadata();
    let initial_states = WorkspaceCrdtDocuments::try_new(&workspace)
        .unwrap()
        .document_states();
    let state = AppState::new(
        workspace,
        settings,
        date(2026, 6, 11),
        date(2026, 6, 11),
        false,
        initial_states,
        1,
    );
    (state, scheme_id)
}

/// Simulate the background half of a sync run: seed a CRDT copy from the
/// snapshot the run captured, merge a "remote" change from another device, and
/// hand back the run's result (merged workspace + final document states).
fn simulated_sync_run_rename(
    snapshot_workspace: &Workspace,
    snapshot_states: &HashMap<DocumentId, Vec<u8>>,
    scheme_id: SchemeId,
    new_name: &str,
) -> (Workspace, HashMap<DocumentId, Vec<u8>>) {
    let other_device = ReplicaId::new();
    let mut run_docs =
        WorkspaceCrdtDocuments::from_states(snapshot_workspace, other_device, snapshot_states)
            .unwrap();
    let mut result_workspace = snapshot_workspace.clone();
    result_workspace.schemes.get_mut(&scheme_id).unwrap().name = new_name.to_string();
    let outcome = run_docs.sync_changes(
        &result_workspace,
        &WorkspaceCrdtChangeSet::default().workspace(),
    );
    assert!(outcome.is_ok(), "{:?}", outcome.errors);
    (result_workspace, run_docs.document_states())
}

#[test]
fn merge_preserves_local_edits_made_during_sync_run() {
    let (mut state, scheme_id) = app_state_with_scheme("Plans");

    // A sync run starts: it snapshots the workspace and document states.
    let watermark = state.local_edit_watermark();
    let snapshot_workspace = state.workspace.clone();
    let snapshot_states = state.crdt_document_states();

    // The run merges a rename made on another device.
    let (result_workspace, result_states) = simulated_sync_run_rename(
        &snapshot_workspace,
        &snapshot_states,
        scheme_id,
        "Renamed remotely",
    );

    // While the run is in flight, the user drafts a calendar event — the item
    // is inserted into the workspace immediately.
    let item = Item::new("Draft event");
    let item_id = item.id;
    state
        .apply_prechecked_local_command(
            Command::InsertItem {
                scheme: scheme_id,
                position: 0,
                item,
            },
            CommandOrigin::User,
        )
        .unwrap();
    assert!(state.has_local_edits_since(watermark));

    // The run lands and is merged instead of replacing the workspace.
    assert!(state.merge_workspace_from_sync(&result_workspace, &result_states));

    let merged_scheme = state.workspace.scheme(scheme_id).unwrap();
    assert_eq!(merged_scheme.name, "Renamed remotely");
    assert!(
        merged_scheme.items.iter().any(|item| item.id == item_id),
        "an item created while the sync run was in flight must survive the merge"
    );
    // The in-flight edit still awaits its own push.
    assert!(state.has_pending_crdt_edits());
}

#[test]
fn merge_preserves_direct_workspace_mutations_made_during_sync_run() {
    let (mut state, scheme_id) = app_state_with_scheme("Plans");

    let watermark = state.local_edit_watermark();
    let snapshot_workspace = state.workspace.clone();
    let snapshot_states = state.crdt_document_states();

    let (result_workspace, result_states) = simulated_sync_run_rename(
        &snapshot_workspace,
        &snapshot_states,
        scheme_id,
        "Renamed remotely",
    );

    // A direct (non-command) mutation while the run is in flight — the path
    // used when e.g. today's Daily Queue scheme is created on the fly.
    let direct = Scheme::new("Direct", 1);
    let direct_id = direct.id;
    state.workspace.schemes.insert(direct_id, direct);
    state
        .workspace
        .folders
        .get_mut(&state.workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(direct_id));
    state.mark_scheme_dirty(direct_id);
    assert!(state.has_local_edits_since(watermark));

    assert!(state.merge_workspace_from_sync(&result_workspace, &result_states));

    assert_eq!(
        state.workspace.scheme(scheme_id).unwrap().name,
        "Renamed remotely"
    );
    assert!(
        state.workspace.scheme(direct_id).is_some(),
        "a scheme created directly while the sync run was in flight must survive the merge"
    );
}

#[test]
fn watermark_reports_no_edits_when_nothing_changed() {
    let (mut state, _) = app_state_with_scheme("Plans");
    let watermark = state.local_edit_watermark();
    assert!(!state.has_local_edits_since(watermark));

    state.mark_direct_workspace_dirty();
    assert!(state.has_local_edits_since(watermark));
}

#[test]
fn merge_rejects_result_with_different_workspace_identity() {
    let (mut state, scheme_id) = app_state_with_scheme("Plans");

    let snapshot_workspace = state.workspace.clone();
    let snapshot_states = state.crdt_document_states();
    let (mut result_workspace, mut result_states) = simulated_sync_run_rename(
        &snapshot_workspace,
        &snapshot_states,
        scheme_id,
        "Renamed remotely",
    );

    // Simulate the run canonicalizing the workspace to a different server
    // identity (first sync after sign-in): the workspace document id changes.
    let old_id = result_workspace.sync.id;
    let new_id = DocumentId::new();
    result_workspace.sync.id = new_id;
    if let Some(workspace_state) = result_states.remove(&old_id) {
        result_states.insert(new_id, workspace_state);
    }

    assert!(
        !state.merge_workspace_from_sync(&result_workspace, &result_states),
        "a result with a different workspace document identity must fall back to the replace path"
    );
}
