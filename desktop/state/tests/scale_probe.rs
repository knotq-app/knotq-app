//! How the hot paths scale on workspaces far larger than a typical one.
//!
//! The existing `edit_cost_probe` measures a *real* workspace, which makes it a
//! good reality check but a poor way to find scaling cliffs — you can only
//! measure the sizes you happen to have. This builds synthetic workspaces at
//! chosen sizes instead, so a cost that grows with the wrong thing (total items
//! rather than edited items, whole text rather than the edit) shows up as a
//! number that moves when it should not.
//!
//! Ignored by default because it is a measurement, not an assertion:
//!
//! ```sh
//! cargo test -p knotq-state --test scale_probe -- --ignored --nocapture
//! ```
//!
//! The one thing here that IS asserted lives in `guards`, at the bottom: cheap
//! upper bounds that catch a regression turning a linear path quadratic.

use std::collections::HashMap;
use std::time::Instant;

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{AppSettings, Item, NodeRef, Scheme, SchemeId, Workspace};
use knotq_state::AppState;
use knotq_sync::WorkspaceCrdtDocuments;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// A workspace with `schemes` schemes of `items_per_scheme` items each, where
/// every item's text is `text_len` characters.
fn synthetic_workspace(schemes: usize, items_per_scheme: usize, text_len: usize) -> Workspace {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let body = "lorem ipsum dolor sit amet ".repeat(text_len / 27 + 1);
    for scheme_index in 0..schemes {
        let mut scheme = Scheme::new(format!("scheme-{scheme_index}"), 0);
        for item_index in 0..items_per_scheme {
            let mut item = Item::new(format!("a{item_index}"));
            item.set_text(format!("{item_index} {}", &body[..text_len.min(body.len())]));
            scheme.items.push(item);
        }
        let id = scheme.id;
        workspace.schemes.insert(id, scheme);
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(id));
    }
    workspace.ensure_sync_metadata();
    workspace
}

fn state_for(workspace: &Workspace) -> AppState {
    let crdt_states = WorkspaceCrdtDocuments::try_new(workspace)
        .expect("build crdt")
        .document_states();
    let today = chrono::Local::now().date_naive();
    AppState::new(
        workspace.clone(),
        AppSettings::default(),
        today,
        today,
        false,
        crdt_states,
        0,
    )
}

fn biggest_scheme(workspace: &Workspace) -> (SchemeId, knotq_model::ItemId, String) {
    let scheme = workspace
        .schemes
        .values()
        .max_by_key(|scheme| scheme.items.len())
        .expect("a scheme");
    let item = &scheme.items[0];
    (scheme.id, item.id, item.text())
}

/// Mean cost of one keystroke (append a character to an item's text).
fn keystroke_ms(state: &mut AppState, workspace: &Workspace, runs: usize) -> f64 {
    let (scheme, item, base) = biggest_scheme(workspace);
    let mut total = 0.0;
    for index in 0..runs {
        let next = format!("{base}{index}");
        let start = Instant::now();
        let _ = state.apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme,
                item,
                text: next,
            },
            CommandOrigin::User,
        );
        total += ms(start);
    }
    total / runs as f64
}

#[test]
#[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
fn probe_scaling() {
    println!();
    println!("== keystroke cost vs workspace size ==");
    println!(
        "{:>8} {:>8} {:>9} | {:>12} {:>12} {:>12}",
        "schemes", "items", "text", "AppState::new", "indexed()", "keystroke"
    );
    for &(schemes, items, text_len) in &[
        (10usize, 100usize, 80usize),
        (100, 100, 80),
        (500, 100, 80),
        (10, 1_000, 80),
        (10, 5_000, 80),
        // Long text: same item count, much longer bodies.
        (10, 100, 2_000),
        (10, 100, 20_000),
    ] {
        let workspace = synthetic_workspace(schemes, items, text_len);
        let start = Instant::now();
        let mut state = state_for(&workspace);
        let construct = ms(start);
        let start = Instant::now();
        let _ = state.indexed();
        let index = ms(start);
        let keystroke = keystroke_ms(&mut state, &workspace, 20);
        println!(
            "{schemes:>8} {items:>8} {text_len:>9} | {construct:>10.1}ms {index:>10.1}ms {keystroke:>10.3}ms"
        );
    }

    println!();
    println!("== sync-path cost vs workspace size ==");
    println!(
        "{:>8} {:>8} | {:>14} {:>16} {:>18}",
        "schemes", "items", "store_from_ws", "document_states", "replace_from_sync"
    );
    for &(schemes, items) in &[(10usize, 100usize), (100, 100), (500, 100), (10, 5_000)] {
        let workspace = synthetic_workspace(schemes, items, 80);
        let mut state = state_for(&workspace);
        let start = Instant::now();
        state.sync_store_from_workspace();
        let store = ms(start);
        let start = Instant::now();
        let states = state.crdt_document_states();
        let states_ms = ms(start);
        // The unchanged case: what a sync run that carried nothing costs.
        let start = Instant::now();
        let changed = state.replace_workspace_from_sync(workspace.clone(), states);
        let replace = ms(start);
        assert!(!changed, "an identical workspace must report no change");
        println!(
            "{schemes:>8} {items:>8} | {store:>12.1}ms {states_ms:>14.1}ms {replace:>16.1}ms"
        );
    }
}

/// Bounds that fail loudly if a hot path stops being linear. Deliberately loose
/// — they are here to catch an accidental quadratic, not to police jitter, and
/// they run on every `cargo test` unlike the probe above.
#[cfg(test)]
mod guards {
    use super::*;

    /// A keystroke must not get dramatically more expensive because OTHER
    /// schemes exist. The edited scheme is identical in both workspaces, so any
    /// large gap is per-keystroke work that scales with the whole workspace.
    #[test]
    fn keystroke_cost_does_not_scale_with_unrelated_schemes() {
        let small = synthetic_workspace(4, 200, 80);
        let large = synthetic_workspace(200, 200, 80);
        let mut small_state = state_for(&small);
        let mut large_state = state_for(&large);

        // Warm both (first edit pays one-off setup on either side).
        let _ = keystroke_ms(&mut small_state, &small, 3);
        let _ = keystroke_ms(&mut large_state, &large, 3);

        let small_ms = keystroke_ms(&mut small_state, &small, 30);
        let large_ms = keystroke_ms(&mut large_state, &large, 30);

        // 50x the schemes must not cost anywhere near 50x per keystroke. The
        // bound is generous so a loaded CI machine cannot flake it; a genuine
        // regression to "rebuild everything per keystroke" blows past it.
        assert!(
            large_ms < small_ms * 8.0 + 1.0,
            "keystroke cost scaled with unrelated schemes: {small_ms:.3}ms with 4 schemes \
             vs {large_ms:.3}ms with 200 (limit {:.3}ms)",
            small_ms * 8.0 + 1.0
        );
    }

    /// A keystroke in a long line must not cost dramatically more than in a
    /// short one beyond the text itself — i.e. no accidental O(text) rebuild of
    /// anything but the text.
    #[test]
    fn keystroke_cost_is_tolerable_on_very_long_text() {
        let short = synthetic_workspace(4, 100, 80);
        let long = synthetic_workspace(4, 100, 40_000);
        let mut short_state = state_for(&short);
        let mut long_state = state_for(&long);
        let _ = keystroke_ms(&mut short_state, &short, 3);
        let _ = keystroke_ms(&mut long_state, &long, 3);

        let short_ms = keystroke_ms(&mut short_state, &short, 20);
        let long_ms = keystroke_ms(&mut long_state, &long, 20);

        assert!(
            long_ms < short_ms * 60.0 + 5.0,
            "keystroke on a 40k-char line cost {long_ms:.3}ms vs {short_ms:.3}ms on an 80-char one"
        );
    }

    /// The no-op sync path (see `replace_workspace_from_sync`) is on every sync
    /// round trip, so its unchanged case must stay cheap on a big workspace.
    #[test]
    fn unchanged_sync_replace_stays_cheap() {
        let workspace = synthetic_workspace(120, 300, 120);
        let mut state = state_for(&workspace);
        let states: HashMap<_, _> = state.crdt_document_states();

        let start = Instant::now();
        let changed = state.replace_workspace_from_sync(workspace.clone(), states);
        let elapsed = ms(start);

        assert!(!changed, "identical workspace must report no visible change");
        assert!(
            elapsed < 750.0,
            "no-op sync replace took {elapsed:.1}ms on 120x300 items"
        );
    }
}

/// Which stage of "open a big scheme" actually costs the time. Separates the
/// CRDT build from the state construction so an optimization aims at the right
/// one.
#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn probe_construction_breakdown() {
    println!();
    println!(
        "{:>7} | {:>12} {:>14} {:>14} {:>12}",
        "items", "crdt::try_new", "document_states", "AppState::new", "total"
    );
    for &items in &[500usize, 1_000, 2_000, 4_000] {
        let workspace = synthetic_workspace(1, items, 80);

        let start = Instant::now();
        let docs = WorkspaceCrdtDocuments::try_new(&workspace).expect("build crdt");
        let build = ms(start);

        let start = Instant::now();
        let states = docs.document_states();
        let dump = ms(start);

        let today = chrono::Local::now().date_naive();
        let start = Instant::now();
        let _state = AppState::new(
            workspace.clone(),
            AppSettings::default(),
            today,
            today,
            false,
            states,
            0,
        );
        let construct = ms(start);

        println!(
            "{items:>7} | {build:>10.1}ms {dump:>12.1}ms {construct:>12.1}ms {:>10.1}ms",
            build + dump + construct
        );
    }
}


/// Where a single keystroke's time goes in a large scheme.
#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn probe_keystroke_breakdown() {
    let workspace = synthetic_workspace(1, 5_000, 80);
    let mut state = state_for(&workspace);
    // Warm: the first edit pays one-off setup.
    let _ = keystroke_ms(&mut state, &workspace, 3);
    println!();
    println!("--- 5 keystrokes on a 5,000-item scheme ---");
    let each = keystroke_ms(&mut state, &workspace, 5);
    println!("mean keystroke: {each:.3}ms");
}

/// Is the residual per-keystroke cost the per-item comparison (which touches
/// each item's text) or something else? Same item count, different text length.
#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn probe_keystroke_vs_text_length() {
    println!();
    println!("{:>7} {:>10} | {:>12} {:>14}", "items", "text len", "keystroke", "per item (us)");
    for &(items, text_len) in &[
        (5_000usize, 10usize),
        (5_000, 80),
        (5_000, 400),
        (500, 80),
        (1_000, 80),
        (2_000, 80),
    ] {
        let workspace = synthetic_workspace(1, items, text_len);
        let mut state = state_for(&workspace);
        let _ = keystroke_ms(&mut state, &workspace, 3);
        let each = keystroke_ms(&mut state, &workspace, 20);
        println!(
            "{items:>7} {text_len:>10} | {each:>10.3}ms {:>12.3}",
            each * 1000.0 / items as f64
        );
    }
}
