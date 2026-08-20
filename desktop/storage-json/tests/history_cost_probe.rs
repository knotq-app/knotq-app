//! What the workspace history store costs the save path.
//!
//! Ignored by default: point `KNOTQ_PROBE_DIR` at a *copy* of a workspace
//! directory and run
//! `cargo test --release -p knotq-storage-json --test history_cost_probe -- --ignored --nocapture`.

use std::time::Instant;

use knotq_model::SchemeId;
use knotq_storage_json::{load_workspace, save_workspace_incremental};

fn median_ms(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

#[test]
#[ignore]
fn probe_history_store_cost() {
    let dir = std::path::PathBuf::from(
        std::env::var("KNOTQ_PROBE_DIR").expect("set KNOTQ_PROBE_DIR to a workspace copy"),
    );
    let path = dir.join("workspace.json");
    let workspace = load_workspace(&path).unwrap().unwrap();
    let first: SchemeId = *workspace.schemes.keys().next().unwrap();
    let dirty = [first].into_iter().collect();

    let history = dir.join(".knotq-history");
    let parked = dir.join(".knotq-history-parked");

    // One save first, so the snapshot cadence is warm and every timing below is
    // a *throttled* save — the common case while typing.
    save_workspace_incremental(&path, &workspace, &dirty).unwrap();

    let mut with = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        save_workspace_incremental(&path, &workspace, &dirty).unwrap();
        with.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    std::fs::rename(&history, &parked).expect("park the history store");
    let mut without = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        save_workspace_incremental(&path, &workspace, &dirty).unwrap();
        without.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    // Whatever the save just recreated is not the store under test.
    let _ = std::fs::remove_dir_all(&history);
    std::fs::rename(&parked, &history).expect("restore the history store");

    let files_before = walkdir_count(&history);
    let bytes_before = walkdir_bytes(&history);

    // The saves above already kicked off the app's own background sweep, so the
    // counts this one reports are only its share of the work; the file counts
    // below are what actually matters.
    let start = Instant::now();
    knotq_storage_json::sweep_workspace_history_now(&dir).expect("sweep");
    let sweep_ms = start.elapsed().as_secs_f64() * 1000.0;

    let mut swept = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        save_workspace_incremental(&path, &workspace, &dirty).unwrap();
        swept.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    println!(
        "history store              {files_before} files, {:.2} GB  ->  {} files, {:.2} GB",
        bytes_before as f64 / 1e9,
        walkdir_count(&history),
        walkdir_bytes(&history) as f64 / 1e9
    );
    println!("sweep took                 {sweep_ms:.0} ms");
    println!("save, unswept store        {:8.1} ms (median of 7)", median_ms(with));
    println!("save, swept store          {:8.1} ms (median of 7)", median_ms(swept));
    println!("save, no store at all      {:8.1} ms (median of 7)", median_ms(without));
}

fn walkdir_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            if entry.path().is_dir() {
                walkdir_bytes(&entry.path())
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

fn walkdir_count(dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            if entry.path().is_dir() {
                walkdir_count(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

/// Where `save_crdt_state`'s time actually goes: encoding 158 documents into
/// one JSON blob, or writing that blob to disk.
#[test]
#[ignore]
fn probe_crdt_state_save_split() {
    use knotq_storage_json::{crdt_state_path, load_crdt_state};

    let dir = std::path::PathBuf::from(std::env::var("KNOTQ_PROBE_DIR").unwrap());
    let path = dir.join("workspace.json");
    let states = load_crdt_state(&path).unwrap();
    let bytes: usize = states.values().map(|s| s.len()).sum();

    let mut encode = Vec::new();
    let mut write = Vec::new();
    let mut json = String::new();
    for _ in 0..7 {
        let start = Instant::now();
        let persisted = knotq_sync::PersistedCrdtState::from_states(&states);
        json = serde_json::to_string(&persisted).unwrap();
        encode.push(start.elapsed().as_secs_f64() * 1000.0);

        let start = Instant::now();
        // Same shape as `write_atomic`: temp file, fsync, rename, fsync parent.
        let target = crdt_state_path(&path);
        let tmp = target.with_extension("probe-tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp).unwrap();
            file.write_all(json.as_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        std::fs::rename(&tmp, &target).unwrap();
        write.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "{} documents, {:.1} MB decoded -> {:.1} MB json",
        states.len(),
        bytes as f64 / 1_048_576.0,
        json.len() as f64 / 1_048_576.0
    );
    println!("encode (base64 + serde)    {:8.1} ms (median of 7)", median_ms(encode));
    println!("write_atomic + fsync       {:8.1} ms (median of 7)", median_ms(write));
}
