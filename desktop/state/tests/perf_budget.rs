//! Performance budgets that CI enforces.
//!
//! Two kinds of assertion, deliberately:
//!
//! * **Shape** tests compare cost at one size against cost at another. They are
//!   profile- and machine-independent — a debug build and a loaded CI runner are
//!   both slow in the *same* proportion — so they run everywhere, on every
//!   `cargo test`, and they are what actually catches an algorithmic regression.
//!   Every number they guard was a real bug: building one scheme's CRDT was
//!   quadratic in its item count (18s for 5,000 items), and a keystroke walked
//!   and re-serialized every item in the scheme.
//!
//! * **Ceiling** tests assert wall-clock milliseconds. Those only mean anything
//!   in release and only on a machine that is not fighting for CPU, so they are
//!   gated behind `KNOTQ_PERF_BUDGET=1` and run in one dedicated CI job. Their
//!   headroom is deliberately large: they exist to catch something becoming
//!   *catastrophically* slow, not to police jitter.
//!
//! Run the ceilings locally with:
//!
//! ```sh
//! KNOTQ_PERF_BUDGET=1 cargo test -p knotq-state --test perf_budget --release
//! ```

use std::time::Instant;

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{AppSettings, Item, NodeRef, Scheme, SchemeId, Workspace};
use knotq_state::AppState;
use knotq_sync::WorkspaceCrdtDocuments;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn ceilings_enabled() -> bool {
    std::env::var("KNOTQ_PERF_BUDGET").is_ok_and(|v| v != "0" && !v.is_empty())
}

fn workspace_of(schemes: usize, items_per_scheme: usize, text_len: usize) -> Workspace {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let body = "lorem ipsum dolor sit amet ".repeat(text_len / 27 + 1);
    for scheme_index in 0..schemes {
        let mut scheme = Scheme::new(format!("scheme-{scheme_index}"), 0);
        for item_index in 0..items_per_scheme {
            let mut item = Item::new(format!("a{item_index}"));
            item.set_text(format!(
                "{item_index} {}",
                &body[..text_len.min(body.len())]
            ));
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

/// Milliseconds to build one scheme's CRDT from scratch.
fn crdt_build_ms(items: usize) -> f64 {
    build_ms(1, items)
}

/// The fastest of three builds of `schemes` x `items_per_scheme`.
///
/// A ratio between two timings only means anything if neither run was
/// preempted, and on a shared CI runner some run always is. The minimum is the
/// one that came closest to having the machine to itself; a single sample is
/// whatever the scheduler happened to allow.
fn build_ms(schemes: usize, items_per_scheme: usize) -> f64 {
    (0..3)
        .map(|_| {
            let workspace = workspace_of(schemes, items_per_scheme, 80);
            let start = Instant::now();
            let _ = WorkspaceCrdtDocuments::try_new(&workspace).expect("build crdt");
            ms(start)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Mean milliseconds for one keystroke in the largest scheme of `workspace`.
fn keystroke_ms(state: &mut AppState, workspace: &Workspace, runs: usize) -> f64 {
    let scheme = workspace
        .schemes
        .values()
        .max_by_key(|scheme| scheme.items.len())
        .expect("a scheme");
    let (scheme_id, item, base): (SchemeId, _, String) =
        (scheme.id, scheme.items[0].id, scheme.items[0].text());
    // Warm: the first edit after construction pays one-off setup (the document
    // read-back that later passes reuse), which is not what we are budgeting.
    for index in 0..3 {
        let _ = state.apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme: scheme_id,
                item,
                text: format!("{base}w{index}"),
            },
            CommandOrigin::User,
        );
    }
    let start = Instant::now();
    for index in 0..runs {
        let _ = state.apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme: scheme_id,
                item,
                text: format!("{base}{index}"),
            },
            CommandOrigin::User,
        );
    }
    ms(start) / runs as f64
}

// ── Shape: these run everywhere ────────────────────────────────────────────

/// Building a scheme's CRDT must stay roughly linear in its item count.
///
/// It was quadratic: 4x the items cost ~16x, so one 5,000-item scheme took 18
/// SECONDS. Three separate causes (a transaction per item, a tail scan per
/// item, and fractional keys that grew linearly on append).
///
/// Both sides build the SAME 4,000 items, differing only in how many schemes
/// they are spread over, so scheme *size* is the only variable and linear means
/// the two timings are EQUAL. Comparing 1,000 items against 4,000 and calling
/// linear "under 8x" was really comparing two different amounts of work behind
/// a fudge factor, and it duly flaked on a Linux runner.
#[test]
fn crdt_build_stays_linear_in_item_count() {
    let spread = build_ms(4, 1_000).max(0.001);
    let concentrated = build_ms(1, 4_000);

    // Equal item counts, so a linear build makes these equal; 3x absorbs the
    // per-scheme overhead the spread side pays four times over. The quadratic
    // this replaced made the concentrated side ~4x slower.
    assert!(
        concentrated < spread * 3.0,
        "CRDT build is superlinear in scheme size: {spread:.1}ms for 4 schemes of 1,000 \
         items vs {concentrated:.1}ms for 1 scheme of 4,000 (same 4,000 items, \
         {:.1}x the time)",
        concentrated / spread
    );
}

/// A keystroke must not get dramatically more expensive as the scheme grows.
///
/// The edit is identical in both cases — one character on one line — so any
/// growth is per-keystroke work that scales with the rest of the scheme.
///
/// This is a RATCHET, not a target. `replace_scheme` still compares every item
/// in the scheme on every keystroke, so the cost is inherently O(items); the
/// measured per-item cost also worsens with size (0.37us at 1,000 items vs
/// 0.66us at 5,000) because touching every item blows the cache. The real fix
/// is to stop touching unchanged items at all — see the note in
/// `scale_probe.rs`. Until then this pins where it is so it cannot slide back,
/// and the bound should be TIGHTENED as that work lands, never loosened.
#[test]
fn keystroke_cost_stays_flat_as_a_scheme_grows() {
    let small_ws = workspace_of(1, 1_000, 80);
    let large_ws = workspace_of(1, 5_000, 80);
    let mut small_state = state_of(&small_ws);
    let mut large_state = state_of(&large_ws);

    let small = keystroke_ms(&mut small_state, &small_ws, 20).max(0.001);
    let large = keystroke_ms(&mut large_state, &large_ws, 20);

    // 5x the items currently costs ~9x the time. Ideally this ratio is ~1 (the
    // work is proportional to what changed). Before this series it was far
    // worse, and a return of the per-item serialization would blow past 15x.
    assert!(
        large < small * 15.0,
        "keystroke cost scales with scheme size: {small:.3}ms at 1,000 items vs \
         {large:.3}ms at 5,000 ({:.1}x for 5x the items)",
        large / small
    );
}

/// A keystroke must not get *super*-linearly worse as unrelated schemes pile up.
///
/// This used to compare 4 schemes of 200 items against 200 schemes of 4 items,
/// which varies scheme count and per-scheme size at the same time. That only
/// isolated scheme count while the per-keystroke CRDT reconciliation dominated
/// the 200-item side; once that reconciliation moved off the keystroke the
/// denominator fell ~5x and the ratio blew past the bound without anything
/// getting slower. Worse, the comparison was hiding what it claimed to measure.
///
/// Measured directly — same 4 items per scheme, only the count varying, release
/// build, ms per keystroke:
///
///        schemes    25      50     100     200     400
///        before   0.022   0.033   0.052   0.089   0.167
///        after    0.009   0.006   0.011   0.023   0.041
///
/// So a keystroke is *linear* in the number of schemes, and always has been —
/// `sync_workspace_from_store_reusing_untouched` rebuilds the whole scheme map
/// on every edit. Deferring the CRDT reconciliation cut the slope about
/// fourfold but did not change its shape. Flat remains the goal; until then this
/// pins the shape, so a change that makes the per-scheme term grow *faster* than
/// linear fails here. TIGHTEN this as that work lands, never loosen it.
#[test]
fn keystroke_cost_grows_no_faster_than_linearly_in_scheme_count() {
    let small_ws = workspace_of(100, 4, 80);
    let large_ws = workspace_of(400, 4, 80);
    let mut small_state = state_of(&small_ws);
    let mut large_state = state_of(&large_ws);

    let small = keystroke_ms(&mut small_state, &small_ws, 20).max(0.001);
    let large = keystroke_ms(&mut large_state, &large_ws, 20);

    // 4x the schemes for 4x the time is the linear line. The headroom absorbs a
    // preempted run on a shared runner; anything quadratic is 16x and cannot
    // hide under it.
    assert!(
        large < small * 8.0,
        "keystroke cost grows faster than linearly in scheme count: {small:.3}ms at \
         100 schemes vs {large:.3}ms at 400 ({:.1}x for 4x the schemes)",
        large / small
    );
}

// ── Ceilings: release + KNOTQ_PERF_BUDGET=1 only ───────────────────────────

/// A keystroke in a very large scheme must stay inside a frame.
#[test]
fn keystroke_stays_within_a_frame_budget() {
    if !ceilings_enabled() {
        return;
    }
    let workspace = workspace_of(1, 5_000, 80);
    let mut state = state_of(&workspace);
    let each = keystroke_ms(&mut state, &workspace, 20);

    // ~3ms locally in release. 16ms is a 60fps frame; a keystroke that costs a
    // whole frame on its own is the regression worth failing CI over, and the
    // gap absorbs a loaded runner.
    assert!(
        each < 16.0,
        "keystroke on a 5,000-item scheme took {each:.2}ms (budget 16ms)"
    );
}

/// Opening a workspace with one very large scheme must not stall.
#[test]
fn large_scheme_crdt_build_stays_within_budget() {
    if !ceilings_enabled() {
        return;
    }
    let each = crdt_build_ms(5_000);
    // ~600ms locally in release, and it was 18,000ms. 6s still fails loudly on
    // a return of the quadratic while tolerating a slow runner.
    assert!(
        each < 6_000.0,
        "building a 5,000-item scheme's CRDT took {each:.0}ms (budget 6000ms)"
    );
}
