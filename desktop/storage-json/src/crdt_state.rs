//! Durable persistence for the long-lived CRDT documents' `state_v1` bytes.
//!
//! The CRDT documents are never rebuilt from plain data with a throwaway identity;
//! they are restored from here (via [`knotq_sync::WorkspaceCrdtDocuments::from_states`])
//! with a deterministic clientID, so their Yjs identity survives app restarts and
//! the desktop UI↔background-thread split.
//!
//! One file per document, in a directory next to `sync-state.json`. The original
//! form was a single JSON blob of base64 documents, which every save had to
//! re-encode and rewrite in full: on a real workspace that is 11.3 MB, costing
//! ~11 ms to encode and ~22 ms to write and fsync — on every save, while a
//! typing pause fires one every two seconds. A save normally touches one or two
//! documents, and per-document files make it cost that instead of the whole set.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use knotq_model::DocumentId;
use knotq_sync::{
    PersistedCrdtState, LOCAL_CRDT_STATE_DIR, LOCAL_CRDT_STATE_EXT, LOCAL_CRDT_STATE_FILE,
};

use crate::sync_state::sync_state_data_dir;

pub fn crdt_state_path(workspace_path: &Path) -> PathBuf {
    sync_state_data_dir(workspace_path).join(LOCAL_CRDT_STATE_FILE)
}

pub fn crdt_state_dir(workspace_path: &Path) -> PathBuf {
    sync_state_data_dir(workspace_path).join(LOCAL_CRDT_STATE_DIR)
}

/// Where the single-blob form is moved once the directory is authoritative.
///
/// Kept rather than deleted: it is the only copy of the pre-migration state, and
/// leaving it under its original name would let an older build load a snapshot
/// that stops being updated the moment this one runs.
pub(crate) fn retired_crdt_state_path(workspace_path: &Path) -> PathBuf {
    sync_state_data_dir(workspace_path).join(format!("{LOCAL_CRDT_STATE_FILE}.migrated"))
}

pub fn load_crdt_state(workspace_path: &Path) -> Result<HashMap<DocumentId, Vec<u8>>> {
    let dir = crdt_state_dir(workspace_path);
    if !dir.is_dir() {
        return load_single_blob(&crdt_state_path(workspace_path));
    }

    let mut states = load_from_dir(&dir)?;
    // The blob is retired only once the whole directory has been written, so
    // finding it still under its own name means the migration did not finish —
    // the device was killed between creating the directory and filling it, or an
    // older build ran in between. Documents the directory already holds are the
    // newer state and win; the rest would otherwise be silently dropped, and a
    // document that comes back absent is rebuilt empty under a fresh identity.
    let legacy_path = crdt_state_path(workspace_path);
    if legacy_path.exists() {
        match load_single_blob(&legacy_path) {
            Ok(legacy) => {
                for (document, bytes) in legacy {
                    states.entry(document).or_insert(bytes);
                }
            }
            // A corrupt blob must not cost the documents that did migrate.
            Err(err) => eprintln!("unmigrated CRDT state blob is unreadable: {err:#}"),
        }
    }
    Ok(states)
}

pub(crate) fn load_from_dir(dir: &Path) -> Result<HashMap<DocumentId, Vec<u8>>> {
    let mut states = HashMap::new();
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some(LOCAL_CRDT_STATE_EXT) {
            // A temp file from an interrupted write, or something not ours.
            continue;
        }
        let Some(document) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<DocumentId>().ok())
        else {
            continue;
        };
        // One unreadable file must cost its own document and no other. Callers
        // load this as `unwrap_or_default()`, so returning `Err` here would not
        // read as "one document is damaged" but as "this device has no CRDT
        // state at all" — every document rebuilt empty, and the account re-seeded
        // from a throwaway identity.
        match fs::read(&path) {
            Ok(bytes) => {
                states.insert(document, bytes);
            }
            Err(err) => eprintln!("skipping unreadable CRDT state {}: {err}", path.display()),
        }
    }
    Ok(states)
}

pub(crate) fn load_single_blob(path: &Path) -> Result<HashMap<DocumentId, Vec<u8>>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(HashMap::new());
    }
    let persisted: PersistedCrdtState =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(persisted.into_states())
}

/// Generic over the byte container so the caller can hand over the shared states
/// from the CRDT documents' encode cache rather than a copy of all of them.
pub fn save_crdt_state<B: AsRef<[u8]>>(
    workspace_path: &Path,
    states: &HashMap<DocumentId, B>,
) -> Result<()> {
    let dir = crdt_state_dir(workspace_path);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    let mut written: HashSet<String> = HashSet::with_capacity(states.len());
    for (document, bytes) in states {
        let name = document_file_name(*document);
        // Unchanged documents — every document but the edited one, normally —
        // cost a read that the page cache serves, instead of a write and two
        // fsyncs.
        crate::files::write_atomic_if_changed(&dir.join(&name), bytes.as_ref())?;
        written.insert(name);
    }

    // A save that carries no documents at all is not a workspace that has none —
    // every workspace has at least its own document. It means the caller's CRDT
    // store was never populated (a failed restore, a save racing startup), and
    // sweeping on the strength of it would delete the identity of every document
    // on disk: they would come back empty under fresh clientIDs and re-seed the
    // account from nothing. Write nothing, remove nothing, keep the old state.
    if states.is_empty() {
        let on_disk = load_from_dir(&dir).map(|states| states.len()).unwrap_or(0);
        if on_disk > 0 {
            eprintln!(
                "refusing to clear {on_disk} persisted CRDT document(s) for a save with no documents"
            );
            return Ok(());
        }
    }

    remove_stale_documents(&dir, &written)?;
    retire_single_blob(workspace_path);
    Ok(())
}

/// Persist only known-changed documents in an already-authoritative per-document
/// state directory. Unlike [`save_crdt_state`], this deliberately does not sweep
/// stale files or retire the legacy blob, so it is safe only for ordinary edits
/// that cannot add or remove documents.
pub fn save_crdt_state_incremental<B: AsRef<[u8]>>(
    workspace_path: &Path,
    states: &HashMap<DocumentId, B>,
) -> Result<()> {
    let dir = crdt_state_dir(workspace_path);
    if !dir.is_dir() || crdt_state_path(workspace_path).exists() {
        bail!("incremental CRDT save requires an authoritative state directory");
    }
    for (document, bytes) in states {
        crate::files::write_atomic_if_changed(
            &dir.join(document_file_name(*document)),
            bytes.as_ref(),
        )?;
    }
    Ok(())
}

pub(crate) fn document_file_name(document: DocumentId) -> String {
    format!("{document}.{LOCAL_CRDT_STATE_EXT}")
}

/// Drops the files of documents that are no longer in the workspace, which the
/// single-blob form got for free by rewriting the whole map.
fn remove_stale_documents(dir: &Path, written: &HashSet<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", dir.display()))?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if written.contains(&name) {
            continue;
        }
        if !name.ends_with(&format!(".{LOCAL_CRDT_STATE_EXT}")) {
            continue;
        }
        fs::remove_file(entry.path())
            .with_context(|| format!("remove {}", entry.path().display()))?;
    }
    Ok(())
}

/// Best effort: the directory is already durable, and failing to move the old
/// blob aside is not a reason to fail a save.
pub(crate) fn retire_single_blob(workspace_path: &Path) {
    let legacy = crdt_state_path(workspace_path);
    if !legacy.exists() {
        return;
    }
    let _ = fs::rename(&legacy, retired_crdt_state_path(workspace_path));
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_workspace(prefix: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("knotq-{prefix}-{}", Uuid::new_v4()))
            .join("workspace")
            .join("workspace.json")
    }

    #[test]
    fn crdt_state_round_trips_through_files() {
        let workspace_path = temp_workspace("crdt-state-test");
        let mut states = HashMap::new();
        let doc = DocumentId::new();
        states.insert(doc, vec![1u8, 2, 3, 255]);

        save_crdt_state(&workspace_path, &states).unwrap();
        let loaded = load_crdt_state(&workspace_path).unwrap();

        assert_eq!(loaded.get(&doc), Some(&vec![1u8, 2, 3, 255]));
        let _ = fs::remove_dir_all(workspace_path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn missing_state_loads_empty() {
        let workspace_path = temp_workspace("crdt-state-missing");
        assert!(load_crdt_state(&workspace_path).unwrap().is_empty());
    }

    /// A workspace written by an older build must come back intact, and the old
    /// blob must stop shadowing the directory once this build has written one.
    #[test]
    fn a_single_blob_workspace_migrates_to_per_document_files() {
        let workspace_path = temp_workspace("crdt-state-migrate");
        let first = DocumentId::new();
        let second = DocumentId::new();
        let legacy = PersistedCrdtState::from_states(&HashMap::from([
            (first, vec![9u8, 8, 7]),
            (second, vec![1u8, 1]),
        ]));
        let path = crdt_state_path(&workspace_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();

        let loaded = load_crdt_state(&workspace_path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(&first), Some(&vec![9u8, 8, 7]));

        save_crdt_state(&workspace_path, &loaded).unwrap();

        assert!(
            !path.exists(),
            "the old blob must not keep shadowing the directory"
        );
        assert!(retired_crdt_state_path(&workspace_path).exists());
        let reloaded = load_crdt_state(&workspace_path).unwrap();
        assert_eq!(reloaded, loaded);

        let _ = fs::remove_dir_all(workspace_path.parent().unwrap().parent().unwrap());
    }

    /// Saving must not do work proportional to the whole workspace: only the
    /// documents whose bytes actually changed may be rewritten.
    #[test]
    fn saving_rewrites_only_the_documents_that_changed() {
        let workspace_path = temp_workspace("crdt-state-incremental");
        let docs: Vec<DocumentId> = (0..8).map(|_| DocumentId::new()).collect();
        let mut states: HashMap<DocumentId, Vec<u8>> = docs
            .iter()
            .map(|doc| (*doc, doc.to_string().into_bytes()))
            .collect();
        save_crdt_state(&workspace_path, &states).unwrap();

        let dir = crdt_state_dir(&workspace_path);
        let modified = |doc: &DocumentId| {
            fs::metadata(dir.join(document_file_name(*doc)))
                .unwrap()
                .modified()
                .unwrap()
        };
        let before: Vec<_> = docs.iter().map(modified).collect();

        std::thread::sleep(std::time::Duration::from_millis(20));
        states.insert(docs[3], b"edited".to_vec());
        save_crdt_state(&workspace_path, &states).unwrap();

        for (i, doc) in docs.iter().enumerate() {
            if i == 3 {
                assert_ne!(
                    modified(doc),
                    before[i],
                    "the edited document must be rewritten"
                );
            } else {
                assert_eq!(
                    modified(doc),
                    before[i],
                    "an unchanged document must not be rewritten"
                );
            }
        }

        let _ = fs::remove_dir_all(workspace_path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn incremental_save_preserves_documents_it_was_not_given() {
        let workspace_path = temp_workspace("crdt-state-partial");
        let kept = DocumentId::new();
        let edited = DocumentId::new();
        save_crdt_state(
            &workspace_path,
            &HashMap::from([(kept, vec![1u8]), (edited, vec![2u8])]),
        )
        .unwrap();

        save_crdt_state_incremental(&workspace_path, &HashMap::from([(edited, vec![3u8])]))
            .unwrap();

        assert_eq!(
            load_crdt_state(&workspace_path).unwrap(),
            HashMap::from([(kept, vec![1u8]), (edited, vec![3u8])])
        );
        let _ = fs::remove_dir_all(workspace_path.parent().unwrap().parent().unwrap());
    }

    /// The safety net. An incremental save cannot create the directory, sweep a
    /// document that went away, or retire the legacy blob — so on a data
    /// directory where any of that is still outstanding it must refuse rather
    /// than write half a state. The store makes the first save of every run a
    /// full one so this never fires in practice; it is here because "never in
    /// practice" is exactly what stops being true.
    #[test]
    fn an_incremental_save_refuses_a_directory_it_cannot_vouch_for() {
        let workspace_path = temp_workspace("crdt-state-nonauthoritative");
        let document = DocumentId::new();

        // No per-document directory yet: a full save has never run here.
        assert!(
            save_crdt_state_incremental(&workspace_path, &HashMap::from([(document, vec![1u8])]))
                .is_err(),
            "an incremental save must refuse before the directory exists"
        );

        save_crdt_state(&workspace_path, &HashMap::from([(document, vec![1u8])])).unwrap();
        // ...and again once the legacy single-blob file is back, because only a
        // full save retires it and a loader that finds it wins over the
        // directory.
        fs::write(crdt_state_path(&workspace_path), "{}").unwrap();
        assert!(
            save_crdt_state_incremental(&workspace_path, &HashMap::from([(document, vec![2u8])]))
                .is_err(),
            "an incremental save must refuse while the legacy blob is still there"
        );
        assert_eq!(
            load_crdt_state(&workspace_path).unwrap(),
            HashMap::from([(document, vec![1u8])]),
            "the refused save must not have written anything"
        );
        let _ = fs::remove_dir_all(workspace_path.parent().unwrap().parent().unwrap());
    }

    /// A document that leaves the workspace must not linger and be restored on
    /// the next launch.
    #[test]
    fn a_removed_document_is_deleted_from_disk() {
        let workspace_path = temp_workspace("crdt-state-removal");
        let kept = DocumentId::new();
        let dropped = DocumentId::new();
        let mut states = HashMap::from([(kept, vec![1u8]), (dropped, vec![2u8])]);
        save_crdt_state(&workspace_path, &states).unwrap();

        states.remove(&dropped);
        save_crdt_state(&workspace_path, &states).unwrap();

        let loaded = load_crdt_state(&workspace_path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key(&kept));
        assert!(!crdt_state_dir(&workspace_path)
            .join(document_file_name(dropped))
            .exists());

        let _ = fs::remove_dir_all(workspace_path.parent().unwrap().parent().unwrap());
    }

    /// An interrupted write leaves a temp file in the directory; loading must
    /// ignore it rather than fail or invent a document.
    #[test]
    fn a_stray_temp_file_is_ignored_by_load() {
        let workspace_path = temp_workspace("crdt-state-temp");
        let doc = DocumentId::new();
        save_crdt_state(&workspace_path, &HashMap::from([(doc, vec![5u8])])).unwrap();
        let dir = crdt_state_dir(&workspace_path);
        fs::write(dir.join("something.ydoc.123.tmp"), b"half written").unwrap();

        let loaded = load_crdt_state(&workspace_path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get(&doc), Some(&vec![5u8]));

        let _ = fs::remove_dir_all(workspace_path.parent().unwrap().parent().unwrap());
    }
}
