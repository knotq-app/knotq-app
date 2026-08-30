//! Deleting lines while a sync run is in flight.
//!
//! A run started before the deletes lands afterwards knowing nothing about
//! them, and the desktop then either merges it into the live documents (local
//! edits outstanding) or replaces the workspace with it (none outstanding).
//! Neither may put a deleted line back.

use std::collections::HashMap;

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{
    AppSettings, DocumentId, Item, ItemId, NodeRef, ReplicaId, Scheme, SchemeId, Workspace,
};
use knotq_state::AppState;
use knotq_sync::{WorkspaceCrdtChangeSet, WorkspaceCrdtDocuments};

mod support;

use support::date;

fn state_with_lines(lines: &[&str]) -> (AppState, SchemeId, Vec<ItemId>) {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Notes", 0);
    let mut ids = Vec::new();
    for line in lines {
        let item = Item::new(*line);
        ids.push(item.id);
        scheme.items.push(item);
    }
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace.ensure_sync_metadata();
    let initial_states = WorkspaceCrdtDocuments::try_new(&workspace)
        .unwrap()
        .document_states();
    let state = AppState::new(
        workspace,
        AppSettings::default(),
        date(2026, 6, 11),
        date(2026, 6, 11),
        false,
        initial_states,
        1,
    );
    (state, scheme_id, ids)
}

fn texts(state: &AppState, scheme: SchemeId) -> Vec<String> {
    state
        .workspace
        .scheme(scheme)
        .unwrap()
        .items
        .iter()
        .map(|item| item.text())
        .collect()
}

/// A sync run that pulled nothing new: it re-encodes the document exactly as
/// the snapshot had it (the self-echo), which is what a run started before the
/// user's deletes hands back.
fn run_echo(
    snapshot_workspace: &Workspace,
    snapshot_states: &HashMap<DocumentId, std::sync::Arc<[u8]>>,
    scheme_id: SchemeId,
) -> (Workspace, HashMap<DocumentId, std::sync::Arc<[u8]>>) {
    let mut run_docs =
        WorkspaceCrdtDocuments::from_states(snapshot_workspace, ReplicaId::new(), snapshot_states)
            .unwrap();
    let result_workspace = snapshot_workspace.clone();
    let outcome = run_docs.sync_changes(
        &result_workspace,
        &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
    );
    assert!(outcome.is_ok(), "{:?}", outcome.errors);
    (result_workspace, run_docs.document_states())
}

fn delete_last(state: &mut AppState, scheme: SchemeId) {
    let id = state
        .workspace
        .scheme(scheme)
        .unwrap()
        .items
        .last()
        .unwrap()
        .id;
    state
        .apply_prechecked_local_command(
            Command::DeleteItem { scheme, item: id },
            CommandOrigin::User,
        )
        .unwrap();
}

/// Deletes land while a run is in flight; the run's result is merged.
#[test]
fn deletes_during_an_in_flight_run_survive_the_merge() {
    let (mut state, scheme, _) = state_with_lines(&["one", "two", "three", "four", "five"]);

    let watermark = state.local_edit_watermark();
    let snapshot = state.workspace.clone();
    let snapshot_states = state.crdt_document_states();
    let (result, result_states) = run_echo(&snapshot, &snapshot_states, scheme);

    for _ in 0..3 {
        delete_last(&mut state, scheme);
    }
    assert_eq!(texts(&state, scheme), ["one", "two"]);
    assert!(state.has_local_edits_since(watermark));

    assert!(state.merge_workspace_from_sync(&result, &result_states));
    assert_eq!(
        texts(&state, scheme),
        ["one", "two"],
        "a line the user deleted came back when the in-flight run landed"
    );
}

/// The same, but each delete gets its own in-flight run landing right after it
/// — the "sync keeps up with the typing" shape over a live socket.
#[test]
fn a_run_landing_between_each_delete_never_resurrects_a_line() {
    let (mut state, scheme, _) = state_with_lines(&["one", "two", "three", "four", "five"]);

    for expected in [
        vec!["one", "two", "three", "four"],
        vec!["one", "two", "three"],
        vec!["one", "two"],
        vec!["one"],
    ] {
        let snapshot = state.workspace.clone();
        let snapshot_states = state.crdt_document_states();
        let (result, result_states) = run_echo(&snapshot, &snapshot_states, scheme);

        delete_last(&mut state, scheme);
        assert_eq!(texts(&state, scheme), expected, "locally");

        state.merge_workspace_from_sync(&result, &result_states);
        assert_eq!(texts(&state, scheme), expected, "after the merge");
    }
}

/// The replace path: the run lands with no local edits outstanding.
#[test]
fn a_run_landing_after_the_deletes_never_resurrects_a_line() {
    let (mut state, scheme, _) = state_with_lines(&["one", "two", "three", "four", "five"]);

    for _ in 0..3 {
        delete_last(&mut state, scheme);
    }
    let snapshot = state.workspace.clone();
    let snapshot_states = state.crdt_document_states();
    let (result, result_states) = run_echo(&snapshot, &snapshot_states, scheme);

    state.replace_workspace_from_sync(result, result_states);
    assert_eq!(texts(&state, scheme), ["one", "two"]);
}

/// Word-by-word backspace then the line merge, with a run in flight throughout.
#[test]
fn word_deletes_then_a_line_merge_during_an_in_flight_run() {
    let (mut state, scheme, ids) = state_with_lines(&["alpha beta", "gamma delta", "epsilon zeta"]);

    let snapshot = state.workspace.clone();
    let snapshot_states = state.crdt_document_states();
    let (result, result_states) = run_echo(&snapshot, &snapshot_states, scheme);

    for text in ["epsilon ", ""] {
        state
            .apply_prechecked_local_command(
                Command::UpdateItemText {
                    scheme,
                    item: ids[2],
                    text: text.into(),
                },
                CommandOrigin::User,
            )
            .unwrap();
    }
    delete_last(&mut state, scheme);
    assert_eq!(texts(&state, scheme), ["alpha beta", "gamma delta"]);

    state.merge_workspace_from_sync(&result, &result_states);
    assert_eq!(
        texts(&state, scheme),
        ["alpha beta", "gamma delta"],
        "the merged line came back"
    );
}
