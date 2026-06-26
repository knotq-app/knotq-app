use knotq_commands::Command;
use knotq_model::{Item, ItemId, Scheme, SchemeId};
use knotq_state::{should_coalesce_editor_undo, AppState, EditorUndoGroup, EditorUndoKey, View};
use std::time::{Duration, Instant};

mod support;

use support::test_state;

fn focus(state: &mut AppState, scheme: SchemeId) {
    state.selection.view = View::Scheme;
    state.selection.scheme_id = Some(scheme);
}

fn focus_union(state: &mut AppState) {
    state.selection.view = View::Union;
    state.selection.scheme_id = None;
}

fn text(state: &AppState, scheme: SchemeId, item: ItemId) -> String {
    state
        .workspace
        .scheme(scheme)
        .and_then(|scheme| scheme.item(item))
        .map(|item| item.text())
        .unwrap_or_default()
}

/// Tiny deterministic PRNG (LCG) so the randomized round-trip test is
/// reproducible without pulling in a `rand`/clock dependency.
fn lcg(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as u32
}

/// Insert a scheme carrying one item directly into the workspace, returning the
/// ids. Mirrors the direct-mutation pattern used by the dispatch tests.
fn add_scheme_with_item(state: &mut AppState, name: &str, body: &str) -> (SchemeId, ItemId) {
    let mut scheme = Scheme::new(name, 0);
    let scheme_id = scheme.id;
    let item = Item::new(body);
    let item_id = item.id;
    scheme.items.push(item);
    state.workspace.schemes.insert(scheme_id, scheme);
    state.mark_scheme_dirty(scheme_id);
    (scheme_id, item_id)
}

#[test]
fn undo_and_redo_restore_workspace_shape() {
    let mut state = test_state();
    let root = state.workspace.root;

    state.apply_command(Command::CreateFolder {
        parent: root,
        name: "Projects".into(),
        position: None,
    });
    assert_eq!(state.workspace.folders[&root].children.len(), 1);

    state.undo_command();
    assert!(state.workspace.folders[&root].children.is_empty());

    state.redo_command();
    assert_eq!(state.workspace.folders[&root].children.len(), 1);
}

#[test]
fn scheme_scoped_undo_isolates_schemes() {
    let mut state = test_state();
    let (a, ia) = add_scheme_with_item(&mut state, "A", "a0");
    let (b, ib) = add_scheme_with_item(&mut state, "B", "b0");

    focus(&mut state, a);
    state.apply_editor_command(Command::UpdateItemText {
        scheme: a,
        item: ia,
        text: "a1".into(),
    });
    focus(&mut state, b);
    state.apply_editor_command(Command::UpdateItemText {
        scheme: b,
        item: ib,
        text: "b1".into(),
    });

    // Undo while focused on A reverts only A; B's newer edit is untouched.
    focus(&mut state, a);
    state.undo_command();
    assert_eq!(text(&state, a, ia), "a0");
    assert_eq!(text(&state, b, ib), "b1");

    // Undo while focused on B reverts B.
    focus(&mut state, b);
    state.undo_command();
    assert_eq!(text(&state, b, ib), "b0");

    // Redo while focused on A reinstates A's edit, leaving B reverted.
    focus(&mut state, a);
    state.redo_command();
    assert_eq!(text(&state, a, ia), "a1");
    assert_eq!(text(&state, b, ib), "b0");
}

#[test]
fn scheme_undo_skips_workspace_ops_then_workspace_undo_reaches_them() {
    let mut state = test_state();
    let (a, ia) = add_scheme_with_item(&mut state, "A", "a0");
    let root = state.workspace.root;

    focus(&mut state, a);
    state.apply_editor_command(Command::UpdateItemText {
        scheme: a,
        item: ia,
        text: "a1".into(),
    });
    // A workspace-structural op lands while the scheme is still focused.
    state.apply_command(Command::CreateFolder {
        parent: root,
        name: "Projects".into(),
        position: None,
    });
    assert_eq!(state.workspace.folders[&root].children.len(), 1);

    // Undo while focused on the scheme reverts the scheme edit, not the folder.
    state.undo_command();
    assert_eq!(text(&state, a, ia), "a0");
    assert_eq!(state.workspace.folders[&root].children.len(), 1);

    // Switching to a no-scheme view lets undo reach the workspace op.
    focus_union(&mut state);
    state.undo_command();
    assert!(state.workspace.folders[&root].children.is_empty());
}

#[test]
fn cross_scheme_undo_skips_when_inverse_no_longer_applies() {
    let mut state = test_state();
    let (a, ia) = add_scheme_with_item(&mut state, "A", "a0");
    let (b, ib) = add_scheme_with_item(&mut state, "B", "b0");

    // One cross-scheme step edits items in both A and B (Global scope).
    focus(&mut state, a);
    state.apply_command(Command::Batch(vec![
        Command::UpdateItemText {
            scheme: a,
            item: ia,
            text: "a1".into(),
        },
        Command::UpdateItemText {
            scheme: b,
            item: ib,
            text: "b1".into(),
        },
    ]));
    assert_eq!(text(&state, a, ia), "a1");
    assert_eq!(text(&state, b, ib), "b1");

    // Later, item B is deleted from B's own view (a scheme-local edit), which
    // invalidates the cross-scheme step's B leg.
    focus(&mut state, b);
    state.apply_command(Command::DeleteItem { scheme: b, item: ib });

    // The cross-scheme step is global, so it's undone from the calendar — but
    // its inverse can no longer apply (B's item is gone). It must be skipped,
    // leaving A's edit intact (no half-undo) rather than corrupting state.
    focus_union(&mut state);
    assert!(state.undo_command().is_none());
    assert_eq!(text(&state, a, ia), "a1");
}

#[test]
fn calendar_initiated_edit_undoes_globally_not_from_scheme() {
    let mut state = test_state();
    let (a, ia) = add_scheme_with_item(&mut state, "A", "a0");

    // An edit made from the calendar (no focused scheme) is a global action that
    // happens to touch a per-scheme item.
    focus_union(&mut state);
    state.apply_command(Command::UpdateItemText {
        scheme: a,
        item: ia,
        text: "fromcal".into(),
    });
    assert_eq!(text(&state, a, ia), "fromcal");

    // Pressing undo inside the scheme does nothing — it's a global action.
    focus(&mut state, a);
    assert!(state.undo_command().is_none());
    assert_eq!(text(&state, a, ia), "fromcal");

    // Pressing undo from the calendar reverts it, and redo reinstates it.
    focus_union(&mut state);
    assert!(state.undo_command().is_some());
    assert_eq!(text(&state, a, ia), "a0");
    assert!(state.redo_command().is_some());
    assert_eq!(text(&state, a, ia), "fromcal");
}

#[test]
fn global_and_scheme_edits_to_same_item_stay_consistent() {
    let mut state = test_state();
    let (a, ia) = add_scheme_with_item(&mut state, "A", "a0");

    // A global (calendar) edit, then a scheme-local edit, both to the same item.
    focus_union(&mut state);
    state.apply_command(Command::UpdateItemText {
        scheme: a,
        item: ia,
        text: "global".into(),
    });
    focus(&mut state, a);
    state.apply_command(Command::UpdateItemText {
        scheme: a,
        item: ia,
        text: "local".into(),
    });
    assert_eq!(text(&state, a, ia), "local");

    // In-scheme undo reverts only the scheme-local edit...
    state.undo_command();
    assert_eq!(text(&state, a, ia), "global");
    // ...and a second in-scheme undo does nothing (the global edit isn't
    // reachable from the scheme).
    assert!(state.undo_command().is_none());
    assert_eq!(text(&state, a, ia), "global");
    // The global edit is undone from the calendar, all the way back to start.
    focus_union(&mut state);
    state.undo_command();
    assert_eq!(text(&state, a, ia), "a0");
}

/// Random per-scheme edits interleaved with random undo/redo must keep every
/// scheme exactly where an independent per-scheme history model says it should
/// be. This is the core "undo/redo always return to the prior state" guarantee
/// plus per-scheme isolation and scoped redo-clearing, all at once.
#[test]
fn randomized_per_scheme_undo_redo_round_trip() {
    for seed in [0x9e3779b9u64, 0x1234_5678, 0xdead_beef, 0xface_cafe, 1, 7, 99] {
        randomized_round_trip_with_seed(seed);
    }
}

fn randomized_round_trip_with_seed(seed: u64) {
    let mut state = test_state();
    let schemes: Vec<(SchemeId, ItemId)> = (0..3)
        .map(|i| add_scheme_with_item(&mut state, &format!("S{i}"), "v0"))
        .collect();

    // Model: each scheme is an independent linear timeline of its item's text.
    let mut history: Vec<Vec<String>> = schemes.iter().map(|_| vec!["v0".to_string()]).collect();
    let mut pos: Vec<usize> = vec![0; schemes.len()];

    let mut rng = seed;
    for step in 0..600u32 {
        let s = (lcg(&mut rng) as usize) % schemes.len();
        let (scheme, item) = schemes[s];
        focus(&mut state, scheme);
        match lcg(&mut rng) % 3 {
            0 => {
                // A fresh edit truncates this scheme's redo future.
                let val = format!("v{step}");
                state.apply_command(Command::UpdateItemText {
                    scheme,
                    item,
                    text: val.clone(),
                });
                history[s].truncate(pos[s] + 1);
                history[s].push(val);
                pos[s] = history[s].len() - 1;
            }
            1 => {
                state.undo_command();
                pos[s] = pos[s].saturating_sub(1);
            }
            _ => {
                state.redo_command();
                if pos[s] + 1 < history[s].len() {
                    pos[s] += 1;
                }
            }
        }

        // Every scheme — not just the one we touched — must match its model.
        for (i, (sc, it)) in schemes.iter().enumerate() {
            assert_eq!(
                text(&state, *sc, *it),
                history[i][pos[i]],
                "divergence at seed {seed:#x}, step {step}, scheme {i}"
            );
        }
    }
}

#[test]
fn editor_undo_coalesces_inside_time_window() {
    let key = EditorUndoKey {
        scheme_id: knotq_model::SchemeId::new(),
        item_id: knotq_model::ItemId::new(),
    };
    let now = Instant::now();
    let group = EditorUndoGroup {
        key,
        last_edit: now - Duration::from_millis(100),
    };

    assert!(should_coalesce_editor_undo(Some(key), Some(group), now));
}
