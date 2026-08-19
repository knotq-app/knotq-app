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

// ---------------------------------------------------------------------------
// Per-scheme narrowing
// ---------------------------------------------------------------------------
//
// `schedule_revision` is workspace-wide, so any schedule change anywhere makes
// the upcoming panel re-expand recurrence for every scheme in the workspace.
// `scheme_schedule_revision` narrows that to the schemes a change actually
// reached. The safe direction is over-reporting: a change whose scope is not
// known must raise *every* scheme's view, so forgetting to narrow costs work
// rather than correctness.

fn state_with_two_schemes() -> (AppState, SchemeId, SchemeId) {
    let mut workspace = Workspace::new();
    let mut ids = Vec::new();
    for name in ["first", "second"] {
        let mut scheme = Scheme::new(name, 0);
        scheme.items = vec![Item::new("alpha"), Item::new("beta")];
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
    let state = AppState::new(
        workspace,
        AppSettings::default(),
        date(2026, 8, 16),
        date(2026, 8, 1),
        false,
        Default::default(),
        1,
    );
    (state, ids[0], ids[1])
}

#[test]
fn a_scheme_scoped_change_leaves_other_schemes_pinned() {
    let (mut state, first, second) = state_with_two_schemes();
    let item = state.workspace.schemes[&first].items[0].id;
    let before_second = state.scheme_schedule_revision(second);
    let before_first = state.scheme_schedule_revision(first);

    state
        .apply_prechecked_local_command(
            Command::SetItemDate {
                scheme: first,
                item,
                kind: DateKind::End,
                date: Some(chrono::Utc::now()),
            },
            CommandOrigin::User,
        )
        .unwrap();

    assert_ne!(
        before_first,
        state.scheme_schedule_revision(first),
        "the edited scheme has to look changed"
    );
    assert_eq!(
        before_second,
        state.scheme_schedule_revision(second),
        "a date set in one scheme must not make every other scheme re-expand"
    );
}

#[test]
fn a_text_edit_pins_every_scheme() {
    let (mut state, first, second) = state_with_two_schemes();
    let item = state.workspace.schemes[&first].items[0].id;
    let before = [
        state.scheme_schedule_revision(first),
        state.scheme_schedule_revision(second),
    ];
    state
        .apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme: first,
                item,
                text: "typed".into(),
            },
            CommandOrigin::User,
        )
        .unwrap();
    assert_eq!(
        before,
        [
            state.scheme_schedule_revision(first),
            state.scheme_schedule_revision(second)
        ],
        "typing must not re-expand recurrence anywhere, not even in its own scheme"
    );
}

/// The invariant the narrowing rests on: a scheme whose contents actually
/// differ after a command must have a moved revision. Comparing the schemes
/// themselves rather than trusting the change set is the point — this is the
/// direction that shows stale rows if it ever breaks.
#[test]
fn every_scheme_that_changed_reports_a_moved_revision() {
    let (mut state, first, second) = state_with_two_schemes();
    let item = state.workspace.schemes[&second].items[0].id;
    let root = state.workspace.root;

    let cases: Vec<(&str, Command)> = vec![
        (
            "SetItemMarker",
            Command::SetItemMarker {
                scheme: second,
                item,
                marker: ItemMarker::Checkbox,
            },
        ),
        (
            "SetItemDate",
            Command::SetItemDate {
                scheme: second,
                item,
                kind: DateKind::Start,
                date: Some(chrono::Utc::now()),
            },
        ),
        (
            "SetItemIndent",
            Command::SetItemIndent {
                scheme: second,
                item,
                indent: 1,
            },
        ),
        (
            "InsertItem",
            Command::InsertItem {
                scheme: second,
                position: 0,
                item: Item::new("new"),
            },
        ),
        (
            "ReorderItem",
            Command::ReorderItem {
                scheme: second,
                from: 0,
                to: 1,
            },
        ),
        ("DeleteItem", Command::DeleteItem { scheme: second, item }),
        (
            "RenameScheme",
            Command::RenameScheme {
                id: second,
                name: "renamed".into(),
            },
        ),
        (
            "SetSchemeColor",
            Command::SetSchemeColor {
                id: second,
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
        ("DeleteScheme", Command::DeleteScheme { id: first }),
    ];

    for (label, command) in cases {
        let before_schemes = state.workspace.schemes.clone();
        let before_revisions: Vec<(SchemeId, u64)> = before_schemes
            .keys()
            .map(|id| (*id, state.scheme_schedule_revision(*id)))
            .collect();

        state
            .apply_prechecked_local_command(command, CommandOrigin::User)
            .unwrap_or_else(|err| panic!("{label}: {err}"));

        for (id, before) in before_revisions {
            let changed = state.workspace.schemes.get(&id) != before_schemes.get(&id);
            if changed {
                assert_ne!(
                    before,
                    state.scheme_schedule_revision(id),
                    "{label} changed scheme {id:?} without moving its revision — \
                     the upcoming panel would keep showing the old rows"
                );
            }
        }
    }
}

/// Deleting a scheme is workspace-level structure, not a scheme-scoped edit: it
/// changes which schemes exist at all, so it has to invalidate everything.
#[test]
fn a_folder_scoped_change_invalidates_every_scheme() {
    let (mut state, first, second) = state_with_two_schemes();
    let before = [
        state.scheme_schedule_revision(first),
        state.scheme_schedule_revision(second),
    ];
    state
        .apply_prechecked_local_command(
            Command::DeleteScheme { id: first },
            CommandOrigin::User,
        )
        .unwrap();
    assert_ne!(before[0], state.scheme_schedule_revision(first));
    assert_ne!(
        before[1],
        state.scheme_schedule_revision(second),
        "a scheme leaving the workspace must invalidate the panel wholesale"
    );
}

/// The direct-mutation and sync routes carry no change set, so they must raise
/// every scheme's view — including one the state has never been asked about.
#[test]
fn scopeless_changes_raise_every_scheme_including_unseen_ones() {
    let (mut state, first, _) = state_with_two_schemes();
    // Never queried, so it has no entry of its own to compare against.
    let unseen = SchemeId::new();

    for (label, mutate) in [
        (
            "mark_direct_workspace_dirty",
            Box::new(|state: &mut AppState| state.mark_direct_workspace_dirty())
                as Box<dyn Fn(&mut AppState)>,
        ),
        (
            "mark_index_dirty",
            Box::new(|state: &mut AppState| state.mark_index_dirty()),
        ),
        (
            "mark_scheme_dirty",
            Box::new(move |state: &mut AppState| state.mark_scheme_dirty(first)),
        ),
        (
            "retained_completed_mut",
            Box::new(|state: &mut AppState| {
                let _ = state.retained_completed_mut();
            }),
        ),
        (
            "replace_workspace",
            Box::new(|state: &mut AppState| {
                state.replace_workspace(Workspace::new(), date(2026, 8, 16), date(2026, 8, 1))
            }),
        ),
    ] {
        let before = state.scheme_schedule_revision(unseen);
        mutate(&mut state);
        assert_ne!(
            before,
            state.scheme_schedule_revision(unseen),
            "{label} carries no scope, so it must invalidate every scheme"
        );
    }
}

/// A scheme's view of the schedule revision must never go backwards, however the
/// scoped and unscoped bumps interleave — a cache comparing for equality would
/// otherwise miss a change that happened to land back on an old value.
#[test]
fn a_schemes_revision_never_goes_backwards() {
    let (mut state, first, second) = state_with_two_schemes();
    let mut highest = [0u64; 2];
    let item = state.workspace.schemes[&first].items[0].id;

    for round in 0..12 {
        if round % 3 == 0 {
            state.mark_direct_workspace_dirty();
        } else {
            state
                .apply_prechecked_local_command(
                    Command::SetItemPriority {
                        scheme: if round % 2 == 0 { first } else { second },
                        item: if round % 2 == 0 {
                            item
                        } else {
                            state.workspace.schemes[&second].items[0].id
                        },
                        priority: Some(round as u8 % 5),
                    },
                    CommandOrigin::User,
                )
                .unwrap();
        }
        for (i, scheme) in [first, second].into_iter().enumerate() {
            let now = state.scheme_schedule_revision(scheme);
            assert!(
                now >= highest[i],
                "round {round}: scheme revision went backwards ({} -> {now})",
                highest[i]
            );
            highest[i] = now;
        }
    }
    assert!(highest[0] > 0 && highest[1] > 0);
}
