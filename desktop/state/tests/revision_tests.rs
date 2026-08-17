//! Tests for the two revision counters views cache against.
//!
//! `content_revision` must move on *any* change to the workspace or the retained
//! completions; `schedule_revision` must move on any change except one that only
//! rewrites item body text. The upcoming panel keeps its (expensive) scan across
//! an unchanged `schedule_revision`, so a change that should have bumped it but
//! did not would leave a stale panel on screen.

use knotq_commands::{Command, CommandOrigin, DateKind};
use knotq_model::{AppSettings, Item, ItemMarker, NodeRef, Scheme, SchemeId, Workspace};
use knotq_state::AppState;

mod support;

use support::date;

fn state_with_scheme() -> (AppState, SchemeId) {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("scheme", 0);
    scheme.items = vec![Item::new("alpha"), Item::new("beta")];
    let scheme_id = scheme.id;
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace.schemes.insert(scheme_id, scheme);
    workspace.ensure_sync_metadata();
    let state = AppState::new(
        workspace,
        AppSettings::default(),
        date(2026, 8, 16),
        date(2026, 8, 1),
        false,
        Default::default(),
        1,
    );
    (state, scheme_id)
}

struct Revisions {
    content: u64,
    schedule: u64,
}

fn revisions(state: &AppState) -> Revisions {
    Revisions {
        content: state.content_revision(),
        schedule: state.schedule_revision(),
    }
}

#[test]
fn a_text_edit_bumps_content_but_not_schedule() {
    let (mut state, scheme) = state_with_scheme();
    let item = state.workspace.schemes[&scheme].items[0].id;
    let before = revisions(&state);
    state
        .apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme,
                item,
                text: "typed".into(),
            },
            CommandOrigin::User,
        )
        .unwrap();
    let after = revisions(&state);
    assert_ne!(
        before.content, after.content,
        "a text edit must bump the content revision — the panel shows item text"
    );
    assert_eq!(
        before.schedule, after.schedule,
        "a text edit cannot change what is scheduled, so it must not invalidate the scan"
    );
}

/// A batch of nothing but text edits is still text-only; a batch with anything
/// else in it is not.
#[test]
fn batch_text_only_classification_is_conservative() {
    let (mut state, scheme) = state_with_scheme();
    let first = state.workspace.schemes[&scheme].items[0].id;
    let second = state.workspace.schemes[&scheme].items[1].id;

    let before = revisions(&state);
    state
        .apply_prechecked_local_command(
            Command::Batch(vec![
                Command::UpdateItemText {
                    scheme,
                    item: first,
                    text: "one".into(),
                },
                Command::UpdateItemText {
                    scheme,
                    item: second,
                    text: "two".into(),
                },
            ]),
            CommandOrigin::User,
        )
        .unwrap();
    assert_eq!(
        before.schedule,
        state.schedule_revision(),
        "an all-text batch must not invalidate the schedule"
    );

    let before = revisions(&state);
    state
        .apply_prechecked_local_command(
            Command::Batch(vec![
                Command::UpdateItemText {
                    scheme,
                    item: first,
                    text: "three".into(),
                },
                Command::SetItemMarker {
                    scheme,
                    item: second,
                    marker: ItemMarker::Checkbox,
                },
            ]),
            CommandOrigin::User,
        )
        .unwrap();
    assert_ne!(
        before.schedule,
        state.schedule_revision(),
        "a batch containing a non-text command must invalidate the schedule"
    );

    // An empty batch claims nothing.
    assert!(!Command::Batch(Vec::new()).changes_only_item_text());
    assert!(!Command::Batch(vec![Command::Batch(Vec::new())]).changes_only_item_text());
}

/// Every command that is *not* a pure text rewrite must move the schedule
/// revision. This is the direction that matters: a missed bump shows stale rows.
#[test]
fn non_text_commands_all_bump_the_schedule_revision() {
    let (mut state, scheme) = state_with_scheme();
    let item = state.workspace.schemes[&scheme].items[0].id;
    let root = state.workspace.root;

    let cases: Vec<(&str, Command)> = vec![
        (
            "SetItemMarker",
            Command::SetItemMarker {
                scheme,
                item,
                marker: ItemMarker::Checkbox,
            },
        ),
        (
            "SetItemDate",
            Command::SetItemDate {
                scheme,
                item,
                kind: DateKind::End,
                date: Some(chrono::Utc::now()),
            },
        ),
        (
            "SetItemPriority",
            Command::SetItemPriority {
                scheme,
                item,
                priority: Some(2),
            },
        ),
        (
            "SetItemIndent",
            Command::SetItemIndent {
                scheme,
                item,
                indent: 1,
            },
        ),
        (
            "InsertItem",
            Command::InsertItem {
                scheme,
                position: 0,
                item: Item::new("new"),
            },
        ),
        (
            "ReorderItem",
            Command::ReorderItem {
                scheme,
                from: 0,
                to: 1,
            },
        ),
        ("DeleteItem", Command::DeleteItem { scheme, item }),
        (
            "RenameScheme",
            Command::RenameScheme {
                id: scheme,
                name: "renamed".into(),
            },
        ),
        (
            "SetSchemeColor",
            Command::SetSchemeColor {
                id: scheme,
                color_index: 3,
            },
        ),
        (
            "CreateScheme",
            Command::CreateScheme {
                folder: root,
                name: "another".into(),
                color_index: 0,
                position: None,
            },
        ),
        (
            "CreateFolder",
            Command::CreateFolder {
                parent: root,
                name: "folder".into(),
                position: None,
            },
        ),
    ];

    for (label, command) in cases {
        assert!(
            !command.changes_only_item_text(),
            "{label} must not be classified as text-only"
        );
        let before = revisions(&state);
        state
            .apply_prechecked_local_command(command, CommandOrigin::User)
            .unwrap_or_else(|err| panic!("{label}: {err}"));
        let after = revisions(&state);
        assert_ne!(
            before.schedule, after.schedule,
            "{label} must bump the schedule revision"
        );
        assert_ne!(
            before.content, after.content,
            "{label} must bump the content revision"
        );
    }
}

/// The direct-mutation routes are how app code changes the workspace outside a
/// command. Each must announce itself on both counters.
#[test]
fn direct_mutation_routes_bump_both_revisions() {
    let (mut state, scheme) = state_with_scheme();

    let before = revisions(&state);
    state.mark_scheme_dirty(scheme);
    assert_ne!(before.content, state.content_revision(), "mark_scheme_dirty");
    assert_ne!(
        before.schedule,
        state.schedule_revision(),
        "mark_scheme_dirty"
    );

    let before = revisions(&state);
    state.mark_index_dirty();
    assert_ne!(before.content, state.content_revision(), "mark_index_dirty");
    assert_ne!(before.schedule, state.schedule_revision(), "mark_index_dirty");

    let before = revisions(&state);
    state.mark_direct_workspace_dirty();
    assert_ne!(
        before.content,
        state.content_revision(),
        "mark_direct_workspace_dirty"
    );
    assert_ne!(
        before.schedule,
        state.schedule_revision(),
        "mark_direct_workspace_dirty"
    );

    let before = revisions(&state);
    state.mark_dirty_from_command(&Command::DeleteScheme { id: scheme });
    assert_ne!(
        before.content,
        state.content_revision(),
        "mark_dirty_from_command"
    );
    assert_ne!(
        before.schedule,
        state.schedule_revision(),
        "mark_dirty_from_command"
    );
}

/// Retained completions feed the panel's "show a just-completed row for a while"
/// behaviour, so taking a mutable handle to them must invalidate the scan.
#[test]
fn touching_retained_completions_bumps_both_revisions() {
    let (mut state, _) = state_with_scheme();
    let before = revisions(&state);
    let _ = state.retained_completed_mut();
    assert_ne!(before.content, state.content_revision());
    assert_ne!(before.schedule, state.schedule_revision());
}

/// A wholesale workspace replacement (how a sync run lands) must invalidate
/// everything.
#[test]
fn replacing_the_workspace_bumps_both_revisions() {
    let (mut state, _) = state_with_scheme();
    let before = revisions(&state);
    state.replace_workspace(Workspace::new(), date(2026, 8, 16), date(2026, 8, 1));
    assert_ne!(before.content, state.content_revision());
    assert_ne!(before.schedule, state.schedule_revision());
}

/// Repeated text edits keep moving the content revision (so the panel refreshes
/// row text) while leaving the schedule revision pinned (so the scan is reused).
#[test]
fn a_typing_burst_pins_the_schedule_revision() {
    let (mut state, scheme) = state_with_scheme();
    let item = state.workspace.schemes[&scheme].items[0].id;
    let schedule_before = state.schedule_revision();
    let mut seen_content = vec![state.content_revision()];
    for i in 0..50 {
        state
            .apply_prechecked_local_command(
                Command::UpdateItemText {
                    scheme,
                    item,
                    text: format!("keystroke {i}"),
                },
                CommandOrigin::User,
            )
            .unwrap();
        seen_content.push(state.content_revision());
    }
    assert_eq!(
        schedule_before,
        state.schedule_revision(),
        "50 keystrokes must not invalidate the upcoming scan even once"
    );
    seen_content.dedup();
    assert_eq!(
        seen_content.len(),
        51,
        "every keystroke must move the content revision so row text refreshes"
    );
}
