//! Tests for the per-command refresh of `AppState::workspace` from the store.
//!
//! Applying a command used to deep-clone the whole workspace back out of the
//! store. It now carries over the `Scheme` values the command's receipt says it
//! did not touch, which is only correct if the local copy ends up byte-identical
//! to the store's — for every command shape, and across sequences of them. These
//! tests assert exactly that, since a divergence here would not fail loudly at
//! runtime: the stale copy would be pushed back into the store by the next
//! direct-mutation flush and the real edit would be lost.

use knotq_commands::{Command, CommandOrigin, DateKind};
use knotq_model::{
    daily_queue_scheme_id, AppSettings, Item, ItemMarker, NodeRef, Scheme, SchemeId, Workspace,
};
use knotq_state::AppState;

mod support;

use support::date;

fn scheme_with_items(name: &str, texts: &[&str]) -> Scheme {
    let mut scheme = Scheme::new(name, 0);
    scheme.items = texts.iter().map(|text| Item::new(*text)).collect();
    scheme
}

/// A workspace with several populated schemes, so a command that touches one of
/// them leaves plenty of untouched content for the reuse path to carry over.
fn state_with_schemes() -> (AppState, Vec<SchemeId>) {
    let mut workspace = Workspace::new();
    let mut ids = Vec::new();
    for i in 0..5 {
        let scheme = scheme_with_items(
            &format!("scheme-{i}"),
            &["alpha", "beta", "gamma", "delta", "epsilon"],
        );
        ids.push(scheme.id);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme.id));
        workspace.schemes.insert(scheme.id, scheme);
    }
    workspace.ensure_sync_metadata();
    let state = AppState::new::<Vec<u8>>(
        workspace,
        AppSettings::default(),
        date(2026, 8, 16),
        date(2026, 8, 1),
        false,
        Default::default(),
        1,
    );
    (state, ids)
}

/// The local copy must equal the store's workspace after `command`.
fn assert_copy_matches_store(state: &mut AppState, label: &str, command: Command) {
    state
        .apply_prechecked_local_command(command, CommandOrigin::User)
        .unwrap_or_else(|err| panic!("{label}: command rejected: {err}"));
    assert_eq!(
        &state.workspace,
        state.store_workspace(),
        "{label}: the local workspace copy diverged from the store"
    );
}

#[test]
fn every_command_shape_leaves_the_copy_matching_the_store() {
    let (mut state, ids) = state_with_schemes();
    let scheme = ids[2];
    let other = ids[0];
    let item = state.workspace.schemes[&scheme].items[1].id;
    let root = state.workspace.root;

    assert_copy_matches_store(
        &mut state,
        "UpdateItemText",
        Command::UpdateItemText {
            scheme,
            item,
            text: "rewritten".into(),
        },
    );
    assert_copy_matches_store(
        &mut state,
        "SetItemMarker",
        Command::SetItemMarker {
            scheme,
            item,
            marker: ItemMarker::Checkbox,
        },
    );
    assert_copy_matches_store(
        &mut state,
        "SetItemDate",
        Command::SetItemDate {
            scheme,
            item,
            kind: DateKind::End,
            date: Some(chrono::Utc::now()),
        },
    );
    assert_copy_matches_store(
        &mut state,
        "SetItemIndent",
        Command::SetItemIndent {
            scheme,
            item,
            indent: 2,
        },
    );
    assert_copy_matches_store(
        &mut state,
        "SetItemPriority",
        Command::SetItemPriority {
            scheme,
            item,
            priority: Some(1),
        },
    );
    assert_copy_matches_store(
        &mut state,
        "InsertItem",
        Command::InsertItem {
            scheme,
            position: 0,
            item: Item::new("inserted"),
        },
    );
    assert_copy_matches_store(
        &mut state,
        "ReorderItem",
        Command::ReorderItem {
            scheme,
            from: 0,
            to: 3,
        },
    );
    let replacement = {
        let mut replacement = state.workspace.schemes[&scheme].items[0].clone();
        replacement.set_text("replaced");
        replacement
    };
    assert_copy_matches_store(
        &mut state,
        "ReplaceItem",
        Command::ReplaceItem {
            scheme,
            item: replacement,
        },
    );
    assert_copy_matches_store(
        &mut state,
        "DeleteItem",
        Command::DeleteItem { scheme, item },
    );

    // Commands that reshape the workspace itself, not just one scheme's items.
    assert_copy_matches_store(
        &mut state,
        "RenameScheme",
        Command::RenameScheme {
            id: scheme,
            name: "renamed".into(),
        },
    );
    assert_copy_matches_store(
        &mut state,
        "SetSchemeColor",
        Command::SetSchemeColor {
            id: scheme,
            color_index: 4,
        },
    );
    assert_copy_matches_store(
        &mut state,
        "CreateFolder",
        Command::CreateFolder {
            parent: root,
            name: "folder".into(),
            position: None,
        },
    );
    let folder = *state
        .workspace
        .folders
        .keys()
        .find(|id| **id != root)
        .expect("folder created");
    assert_copy_matches_store(
        &mut state,
        "MoveNode",
        Command::MoveNode {
            node: NodeRef::Scheme(other),
            new_parent: folder,
            position: 0,
        },
    );
    assert_copy_matches_store(
        &mut state,
        "RenameFolder",
        Command::RenameFolder {
            id: folder,
            name: "renamed folder".into(),
        },
    );
    assert_copy_matches_store(
        &mut state,
        "SetFolderExpanded",
        Command::SetFolderExpanded {
            id: folder,
            expanded: false,
        },
    );
    assert_copy_matches_store(
        &mut state,
        "CreateScheme",
        Command::CreateScheme {
            folder: root,
            name: "new scheme".into(),
            color_index: 1,
            position: None,
        },
    );
    assert_copy_matches_store(
        &mut state,
        "DeleteScheme",
        Command::DeleteScheme { id: ids[4] },
    );
    assert_copy_matches_store(
        &mut state,
        "PermanentlyDeleteScheme",
        Command::PermanentlyDeleteScheme { id: ids[4] },
    );
    assert_copy_matches_store(
        &mut state,
        "DeleteFolder",
        Command::DeleteFolder { id: folder },
    );
}

/// A batch touching several schemes at once must carry over only the schemes
/// outside the batch.
#[test]
fn a_batch_across_several_schemes_leaves_the_copy_matching_the_store() {
    let (mut state, ids) = state_with_schemes();
    let commands = ids[..3]
        .iter()
        .map(|scheme| Command::UpdateItemText {
            scheme: *scheme,
            item: state.workspace.schemes[scheme].items[0].id,
            text: format!("batched {scheme}"),
        })
        .collect::<Vec<_>>();
    assert_copy_matches_store(
        &mut state,
        "Batch",
        Command::from_vec(commands).expect("non-empty batch"),
    );
    for scheme in &ids[..3] {
        assert_eq!(
            state.workspace.schemes[scheme].items[0].text(),
            format!("batched {scheme}")
        );
    }
}

/// The untouched schemes must be carried over *as content*, not merely be equal
/// by luck: edit one scheme repeatedly and check the others still read correctly
/// and that a later edit to one of them still lands.
#[test]
fn carried_over_schemes_stay_readable_and_editable() {
    let (mut state, ids) = state_with_schemes();
    for i in 0..25 {
        let item = state.workspace.schemes[&ids[0]].items[0].id;
        state
            .apply_prechecked_local_command(
                Command::UpdateItemText {
                    scheme: ids[0],
                    item,
                    text: format!("edit {i}"),
                },
                CommandOrigin::User,
            )
            .unwrap();
    }
    for scheme in &ids[1..] {
        assert_eq!(
            state.workspace.schemes[scheme]
                .items
                .iter()
                .map(|item| item.text())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta", "gamma", "delta", "epsilon"],
            "an untouched scheme lost content while another was edited"
        );
    }

    // A scheme that was carried over many times must still be editable.
    let item = state.workspace.schemes[&ids[3]].items[2].id;
    state
        .apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme: ids[3],
                item,
                text: "late edit".into(),
            },
            CommandOrigin::User,
        )
        .unwrap();
    assert_eq!(state.workspace.schemes[&ids[3]].items[2].text(), "late edit");
    assert_eq!(&state.workspace, state.store_workspace());
}

/// A direct mutation of the local copy is flushed into the store by the next
/// command. The reuse path must not resurrect the pre-mutation scheme.
#[test]
fn a_direct_mutation_survives_the_next_command() {
    let (mut state, ids) = state_with_schemes();
    state
        .workspace
        .schemes
        .get_mut(&ids[1])
        .unwrap()
        .items
        .push(Item::new("added directly"));
    state.mark_scheme_dirty(ids[1]);

    let item = state.workspace.schemes[&ids[0]].items[0].id;
    state
        .apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme: ids[0],
                item,
                text: "unrelated".into(),
            },
            CommandOrigin::User,
        )
        .unwrap();

    assert_eq!(
        state.workspace.schemes[&ids[1]].items.last().unwrap().text(),
        "added directly",
        "a direct mutation was lost when an unrelated command refreshed the copy"
    );
    assert_eq!(&state.workspace, state.store_workspace());
}

/// The daily queue is stored as a scheme like any other, and its ids are derived
/// rather than random — exercise it explicitly since it has its own sync binding.
#[test]
fn daily_queue_edits_leave_the_copy_matching_the_store() {
    let (mut state, _) = state_with_schemes();
    let day = date(2026, 8, 16);
    let daily = daily_queue_scheme_id(day);
    let mut scheme = scheme_with_items("Daily", &["one", "two"]);
    scheme.id = daily;
    state.workspace.schemes.insert(daily, scheme);
    state.workspace.daily_queue.insert(day, daily);
    state.mark_index_dirty();

    let item = state.workspace.schemes[&daily].items[0].id;
    assert_copy_matches_store(
        &mut state,
        "daily queue UpdateItemText",
        Command::UpdateItemText {
            scheme: daily,
            item,
            text: "edited daily row".into(),
        },
    );
    assert_eq!(
        state.workspace.schemes[&daily].items[0].text(),
        "edited daily row"
    );
}
