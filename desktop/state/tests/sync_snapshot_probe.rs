//! What one sync run's UI-thread snapshot costs, as a function of how many
//! keystrokes it has to reconcile. Probe, not a test.
//!
//! This is the measurement behind `SYNC_LOCAL_CHANGE_DEBOUNCE_WS`: a shorter
//! debounce means more snapshots per second of typing, each carrying fewer
//! deferred edits, so the question is whether the per-snapshot cost is mostly
//! fixed (shorter debounce = more total main-thread work) or mostly per-edit
//! (shorter debounce = the same work, paid earlier).
//!
//! `cargo test -p knotq-state --test sync_snapshot_probe --release -- --ignored --nocapture`

use std::time::Instant;

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{AppSettings, Item, ItemId, NodeRef, Scheme, SchemeId, Workspace};
use knotq_state::AppState;
use knotq_sync::WorkspaceCrdtDocuments;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn workspace_of(schemes: usize, items: usize) -> (Workspace, Vec<(SchemeId, ItemId)>) {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let mut targets = Vec::new();
    for s in 0..schemes {
        let mut scheme = Scheme::new(format!("Scheme {s}"), 0);
        for i in 0..items {
            scheme.items.push(Item::new(format!(
                "scheme {s} line {i} with some body text"
            )));
        }
        targets.push((scheme.id, scheme.items[0].id));
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme.id));
        workspace.schemes.insert(scheme.id, scheme);
    }
    workspace.ensure_sync_metadata();
    (workspace, targets)
}

fn state_of(workspace: &Workspace) -> AppState {
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

/// One keystroke into `scheme`/`item`, the way the editor issues it.
fn keystroke(state: &mut AppState, scheme: SchemeId, item: ItemId, text: &str) {
    let _ = state.apply_prechecked_local_command(
        Command::UpdateItemText {
            scheme,
            item,
            text: text.to_string(),
        },
        CommandOrigin::User,
    );
}

/// The block `run_sync_attempt` runs on the UI thread before handing the
/// snapshot to the background executor.
fn snapshot(state: &mut AppState, workspace: &Workspace) -> (f64, f64, f64, f64) {
    let t = Instant::now();
    state.sync_store_from_workspace();
    let store = ms(t);
    let t = Instant::now();
    let pending = state.pending_crdt_edits();
    let pending_ms = ms(t);
    let t = Instant::now();
    let handles = state.crdt_document_state_handles();
    let handles_ms = ms(t);
    let t = Instant::now();
    let cloned = workspace.clone();
    let clone_ms = ms(t);
    std::hint::black_box((pending, handles, cloned));
    (store, pending_ms, handles_ms, clone_ms)
}

/// How a burst of keystrokes is spread over the workspace.
#[derive(Clone, Copy)]
enum Burst {
    /// Every keystroke lands in one line — typing a sentence.
    OneLine,
    /// Each keystroke lands in a different line of one scheme — the worst case
    /// for coalescing, since the reconcile has more dirty items to walk.
    WalkingLines,
}

fn run(label: &str, schemes: usize, items: usize, burst: Burst) {
    const KEYSTROKES: usize = 400;

    let (workspace, targets) = workspace_of(schemes, items);
    println!(
        "\n{label}: {schemes} schemes x {items} items = {} items",
        schemes * items
    );
    println!(
        "{:>6}  {:>5}  {:>9}  {:>9}  {:>8}  {:>8}  {:>8}  {:>8}",
        "edits", "runs", "total ms", "per run", "store", "pending", "handles", "clone"
    );

    for edits_per_run in [1usize, 2, 4, 8, 16, 32] {
        let mut state = state_of(&workspace);
        let scheme = targets[0].0;
        let lines: Vec<ItemId> = workspace.schemes[&scheme]
            .items
            .iter()
            .map(|item| item.id)
            .collect();
        let pick = |index: usize| match burst {
            Burst::OneLine => lines[0],
            Burst::WalkingLines => lines[index % lines.len()],
        };
        // Warm: the first snapshot after construction pays one-off setup.
        for i in 0..8 {
            keystroke(&mut state, scheme, pick(i), &format!("warm {i}"));
        }
        let _ = snapshot(&mut state, &workspace);

        let (mut store, mut pending, mut handles, mut clone) = (0.0, 0.0, 0.0, 0.0);
        let mut runs = 0usize;
        for i in 0..KEYSTROKES {
            keystroke(&mut state, scheme, pick(i), &format!("typing {i}"));
            if (i + 1) % edits_per_run == 0 {
                let (s, p, h, c) = snapshot(&mut state, &workspace);
                store += s;
                pending += p;
                handles += h;
                clone += c;
                runs += 1;
            }
        }
        let total = store + pending + handles + clone;
        let per_run = total / runs as f64;
        println!(
            "{edits_per_run:>6}  {runs:>5}  {total:>9.1}  {per_run:>9.3}  {store:>8.1}  \
             {pending:>8.1}  {handles:>8.1}  {clone:>8.1}",
        );
    }
}

#[test]
#[ignore]
fn snapshot_cost_by_edits_per_run() {
    println!(
        "totals are across 400 keystrokes, so rows compare directly: same typing, \
         different debounce. At ~10 keystrokes/sec, 500ms of typing is ~5 edits \
         per run and 2s is ~20."
    );
    run("one line", 180, 11, Burst::OneLine);
    run("walking lines", 180, 11, Burst::WalkingLines);
    run("one line, larger", 170, 22, Burst::OneLine);
    run("walking lines, larger", 170, 22, Burst::WalkingLines);
}
