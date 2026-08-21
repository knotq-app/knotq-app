//! Shared helpers for the upgrade tests.

use std::fs;
use std::path::{Path, PathBuf};

/// Every captured data directory under `tests/fixtures/`, oldest release first.
///
/// Each one was written by running that release's own code (see
/// `tests/fixtures/README.md`), so these are the bytes a real install of that
/// version has on disk — not this build's idea of what it used to write.
pub fn release_fixtures() -> Vec<(String, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(&root)
        .unwrap_or_else(|err| panic!("read {}: {err}", root.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            name.starts_with('v').then_some((name, path))
        })
        .collect();
    found.sort();
    assert!(
        !found.is_empty(),
        "no release fixtures in {} — the upgrade tests would pass by testing nothing",
        root.display()
    );
    found
}

/// Copy a fixture somewhere writable. Returns the temp data directory.
pub fn open_fixture(fixture: &Path, label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "knotq-upgrade-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    copy_dir(fixture, &dir);
    dir
}

pub fn workspace_path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("workspace").join("workspace.json")
}

pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Every file under `dir`, relative, with its bytes — for asserting that
/// something left the directory alone.
pub fn snapshot_tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    collect(dir, dir, &mut out);
    out.sort();
    return out;

    fn collect(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((path.strip_prefix(root).unwrap().to_path_buf(), bytes));
            }
        }
    }
}

/// Best effort: a background history sweep may still be writing in here, and on
/// Linux that makes the delete fail. Cleanup is not what these tests assert.
pub fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}
