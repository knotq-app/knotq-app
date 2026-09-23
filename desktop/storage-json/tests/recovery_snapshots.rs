//! A snapshot is only worth having if putting it back actually restores the
//! workspace, so these tests are about the *pair* of operations, not the copy.

use std::fs;
use std::path::{Path, PathBuf};

use knotq_storage_json::{
    capture_daily_snapshot, list_snapshots, pending_restore, request_restore, restore_snapshot,
    take_pending_restore,
};

/// A writable data directory of its own, following the convention in
/// `tests/support`: no `tempfile` dependency, unique per process and test.
fn temp_data_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "knotq-snapshot-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// A data directory with both halves populated: the plain files the app reads
/// and the CRDT states sync merges into.
fn data_dir_with_both_halves(root: &Path) {
    write(&root.join("workspace.json"), r#"{"version":1}"#);
    write(&root.join("settings.json"), r#"{"theme":"Obsidian"}"#);
    write(&root.join("schemes/one.knotq"), "first scheme");
    write(&root.join("daily_queue/2026/09/21.knotq"), "a day");
    write(&root.join("sync-crdt-state/doc-a.bin"), "crdt bytes");
    write(&root.join("sync-state.json"), r#"{"pending":[]}"#);
    write(&root.join("assets/images/pic.png"), "png bytes");
}

#[test]
fn a_snapshot_carries_the_crdt_half_too() {
    const LABEL: &str = "a_snapshot_carries_the_crdt_half_too";
    let root = &temp_data_dir(LABEL);
    data_dir_with_both_halves(root);

    let snapshot = capture_daily_snapshot(root)
        .unwrap()
        .expect("first call captures");

    // The plain half alone is not restorable: the launch reconcile rebuilds the
    // workspace FROM the documents, so a snapshot without them is undone the
    // moment it is put back. Both halves, or it is not a recovery point.
    assert_eq!(
        fs::read_to_string(snapshot.dir.join("workspace.json")).unwrap(),
        r#"{"version":1}"#
    );
    assert_eq!(
        fs::read_to_string(snapshot.dir.join("sync-crdt-state/doc-a.bin")).unwrap(),
        "crdt bytes"
    );
    assert_eq!(
        fs::read_to_string(snapshot.dir.join("sync-state.json")).unwrap(),
        r#"{"pending":[]}"#
    );
    assert_eq!(
        fs::read_to_string(snapshot.dir.join("daily_queue/2026/09/21.knotq")).unwrap(),
        "a day"
    );
    assert_eq!(
        fs::read_to_string(snapshot.dir.join("settings.json")).unwrap(),
        r#"{"theme":"Obsidian"}"#
    );
}

#[test]
fn the_days_snapshot_is_taken_once() {
    const LABEL: &str = "the_days_snapshot_is_taken_once";
    let root = &temp_data_dir(LABEL);
    data_dir_with_both_halves(root);

    assert!(capture_daily_snapshot(root).unwrap().is_some());
    // Re-taking it on every save would both cost the save and overwrite the
    // morning's copy with the damage the user wants to undo.
    assert!(capture_daily_snapshot(root).unwrap().is_none());
    assert_eq!(list_snapshots(root).len(), 1);
}

#[test]
fn restoring_puts_both_halves_back_and_keeps_what_it_replaced() {
    const LABEL: &str = "restoring_puts_both_halves_back_and_keeps_what_it_replaced";
    let root = &temp_data_dir(LABEL);
    data_dir_with_both_halves(root);
    let snapshot = capture_daily_snapshot(root).unwrap().unwrap();

    // The workspace moves on, and a file appears that the snapshot never had.
    write(&root.join("workspace.json"), r#"{"version":2}"#);
    write(&root.join("sync-crdt-state/doc-a.bin"), "newer crdt bytes");
    write(&root.join("schemes/two.knotq"), "a scheme added later");

    let displaced = restore_snapshot(root, &snapshot).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("workspace.json")).unwrap(),
        r#"{"version":1}"#
    );
    assert_eq!(
        fs::read_to_string(root.join("sync-crdt-state/doc-a.bin")).unwrap(),
        "crdt bytes"
    );
    // A file the snapshot does not have must not survive: leaving it behind is
    // how the restored directory ends up describing two different workspaces.
    assert!(!root.join("schemes/two.knotq").exists());

    // Restoring the wrong day is itself recoverable.
    assert_eq!(
        fs::read_to_string(displaced.join("workspace.json")).unwrap(),
        r#"{"version":2}"#
    );
    assert_eq!(
        fs::read_to_string(displaced.join("schemes/two.knotq")).unwrap(),
        "a scheme added later"
    );
}

#[test]
fn snapshots_do_not_contain_snapshots() {
    const LABEL: &str = "snapshots_do_not_contain_snapshots";
    let root = &temp_data_dir(LABEL);
    data_dir_with_both_halves(root);

    capture_daily_snapshot(root).unwrap().unwrap();
    // Copying the snapshot root into the next snapshot grows the directory
    // geometrically and eventually fills the disk.
    let snapshot = list_snapshots(root).into_iter().next().unwrap();
    assert!(!snapshot.dir.join("snapshots").exists());
    assert!(!snapshot.dir.join("logs").exists());
}

#[test]
fn an_interrupted_snapshot_is_not_offered_as_one() {
    const LABEL: &str = "an_interrupted_snapshot_is_not_offered_as_one";
    let root = &temp_data_dir(LABEL);
    data_dir_with_both_halves(root);

    // What a crash mid-copy leaves behind. A half-copied directory presented as
    // a recovery point is worse than having none.
    write(
        &root.join("snapshots/.2026-09-20.partial/workspace.json"),
        "half written",
    );
    let listed = list_snapshots(root);
    assert!(
        listed.iter().all(|entry| !entry.day.contains("partial")),
        "listed a staging directory: {listed:?}"
    );
}

#[test]
fn a_requested_restore_happens_at_the_next_launch() {
    const LABEL: &str = "a_requested_restore_happens_at_the_next_launch";
    let root = &temp_data_dir(LABEL);
    data_dir_with_both_halves(root);
    let snapshot = capture_daily_snapshot(root).unwrap().unwrap();

    write(&root.join("workspace.json"), r#"{"version":2}"#);
    request_restore(root, &snapshot).unwrap();
    assert_eq!(
        pending_restore(root).as_deref(),
        Some(snapshot.day.as_str())
    );

    take_pending_restore(root)
        .expect("a restore was pending")
        .unwrap();

    assert_eq!(
        fs::read_to_string(root.join("workspace.json")).unwrap(),
        r#"{"version":1}"#
    );
    // Cleared, so the next launch is an ordinary one.
    assert!(pending_restore(root).is_none());
    assert!(take_pending_restore(root).is_none());
}

#[test]
fn a_restore_is_attempted_once_even_if_it_fails() {
    const LABEL: &str = "a_restore_is_attempted_once_even_if_it_fails";
    let root = &temp_data_dir(LABEL);
    data_dir_with_both_halves(root);
    let snapshot = capture_daily_snapshot(root).unwrap().unwrap();
    request_restore(root, &snapshot).unwrap();
    // The snapshot goes away between the request and the launch.
    fs::remove_dir_all(&snapshot.dir).unwrap();

    // No snapshot to restore, so nothing is attempted — and crucially the
    // request is not left behind to retry against a directory that has moved
    // on again.
    assert!(take_pending_restore(root).is_none());
    assert!(pending_restore(root).is_none());
}

#[test]
fn a_restore_request_is_not_mistaken_for_a_snapshot() {
    const LABEL: &str = "a_restore_request_is_not_mistaken_for_a_snapshot";
    let root = &temp_data_dir(LABEL);
    data_dir_with_both_halves(root);
    let snapshot = capture_daily_snapshot(root).unwrap().unwrap();
    request_restore(root, &snapshot).unwrap();

    assert_eq!(list_snapshots(root).len(), 1);
}
