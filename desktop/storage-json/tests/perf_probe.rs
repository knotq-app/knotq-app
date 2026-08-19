//! Timings for the persistence path against a real workspace.
//!
//! Ignored by default: point `KNOTQ_PROBE_DIR` at a *copy* of a workspace
//! directory (the one holding `workspace.json`) and run
//! `cargo test -p knotq-storage-json --test perf_probe -- --ignored --nocapture`.

use std::time::Instant;

use knotq_model::SchemeId;
use knotq_storage_json::{
    load_crdt_state, load_workspace, save_crdt_state, save_workspace, save_workspace_incremental,
};

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore]
fn probe_persistence_costs() {
    let dir = std::path::PathBuf::from(
        std::env::var("KNOTQ_PROBE_DIR").expect("set KNOTQ_PROBE_DIR to a workspace copy"),
    );
    let path = dir.join("workspace.json");

    let start = Instant::now();
    let workspace = load_workspace(&path).expect("load workspace").expect("workspace present");
    println!(
        "load_workspace                {:8.1} ms   ({} schemes, {} items)",
        ms(start),
        workspace.schemes.len(),
        workspace
            .schemes
            .values()
            .map(|scheme| scheme.items.len())
            .sum::<usize>()
    );

    let start = Instant::now();
    let clone = workspace.clone();
    println!("workspace.clone()             {:8.1} ms", ms(start));
    drop(clone);

    // Warm the history store's cadence: the first save always snapshots.
    let start = Instant::now();
    save_workspace(&path, &workspace).expect("save");
    println!("save_workspace (cold history) {:8.1} ms", ms(start));

    let start = Instant::now();
    save_workspace(&path, &workspace).expect("save");
    println!("save_workspace (warm)         {:8.1} ms", ms(start));

    let first: SchemeId = *workspace.schemes.keys().next().expect("a scheme");
    let start = Instant::now();
    save_workspace_incremental(&path, &workspace, &[first].into_iter().collect())
        .expect("incremental save");
    println!("save_workspace_incremental    {:8.1} ms   (1 scheme)", ms(start));

    let start = Instant::now();
    knotq_storage_json::record_workspace_snapshot(&dir).expect("history snapshot");
    println!("record_workspace_snapshot     {:8.1} ms   (throttled: may be a no-op)", ms(start));

    // The CRDT state sits beside the workspace directory, and both the restore at
    // startup and every save go through it.
    let start = Instant::now();
    let states = load_crdt_state(&path).expect("load crdt state");
    let bytes: usize = states.values().map(|state| state.len()).sum();
    println!(
        "load_crdt_state               {:8.1} ms   ({} documents, {:.1} MB decoded)",
        ms(start),
        states.len(),
        bytes as f64 / 1_048_576.0
    );

    let start = Instant::now();
    save_crdt_state(&path, &states).expect("save crdt state");
    println!("save_crdt_state               {:8.1} ms   (every save)", ms(start));
}
