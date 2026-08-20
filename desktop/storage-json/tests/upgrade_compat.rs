//! What an existing install must survive when it opens this build for the first
//! time.
//!
//! The per-document CRDT state directory (`sync-crdt-state/`) replaced a single
//! `sync-crdt-state.json` blob. Every device that has ever synced holds that blob,
//! and the documents' Yjs identity lives in it: lose it and the device rebuilds
//! its documents empty, re-seeds the server under a throwaway identity, and the
//! account diverges. These tests pin the upgrade paths that reach that state —
//! including the ones where the migration does not run to completion.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use knotq_model::{AppSettings, DocumentId, Item, NodeRef, ReplicaId, Scheme, Workspace};
use knotq_storage_json::{
    crdt_state_dir, crdt_state_path, load_app_settings, load_crdt_state, save_crdt_state,
};
use knotq_sync::{PersistedCrdtState, WorkspaceCrdtDocuments};

fn unique_workspace_path(prefix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path.join("workspace.json")
}

fn workspace_with_schemes(names: &[&str]) -> Workspace {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    for name in names {
        let mut scheme = Scheme::new(*name, 2);
        scheme.items.push(Item::new(format!("{name} first line")));
        scheme.items.push(Item::new(format!("{name} second line")));
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));
    }
    workspace.ensure_sync_metadata();
    workspace
}

/// Write the states the way every pre-`sync-crdt-state/` build wrote them.
fn write_legacy_blob<B: AsRef<[u8]>>(workspace_path: &Path, states: &HashMap<DocumentId, B>) {
    let owned: HashMap<DocumentId, Vec<u8>> = states
        .iter()
        .map(|(id, bytes)| (*id, bytes.as_ref().to_vec()))
        .collect();
    let path = crdt_state_path(workspace_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let blob = PersistedCrdtState::from_states(&owned);
    fs::write(&path, serde_json::to_string(&blob).unwrap()).unwrap();
}

fn cleanup(workspace_path: &Path) {
    let _ = fs::remove_dir_all(workspace_path.parent().unwrap());
}

/// The upgrade in full: a device whose only CRDT state is the legacy blob must
/// come up with *the same documents*, not rebuilt ones. Identity is the thing —
/// same document ids, same authoring clientIDs, byte-identical states — because
/// a document that comes back with a fresh identity is what wedges an account.
#[test]
fn an_existing_install_keeps_every_documents_identity_across_the_migration() {
    let workspace_path = unique_workspace_path("knotq-upgrade-identity");
    let workspace = workspace_with_schemes(&["Notes", "Health", "PhD"]);
    let replica = ReplicaId::new();

    let before = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
    let before_states = before.document_states();
    let before_ids = before.known_document_ids();
    write_legacy_blob(&workspace_path, &before_states);

    // First launch on this build: read the blob, restore, then save — which is
    // what performs the migration.
    let loaded = load_crdt_state(&workspace_path).unwrap();
    assert_eq!(
        loaded.len(),
        before_states.len(),
        "every persisted document must survive the read"
    );
    let restored = WorkspaceCrdtDocuments::from_states(&workspace, replica, &loaded).unwrap();
    save_crdt_state(&workspace_path, &restored.document_states()).unwrap();

    assert!(
        crdt_state_dir(&workspace_path).is_dir(),
        "the save must have written the per-document directory"
    );
    assert!(
        !crdt_state_path(&workspace_path).exists(),
        "the legacy blob must not keep shadowing the directory"
    );

    // Second launch: from the directory alone.
    let reloaded = load_crdt_state(&workspace_path).unwrap();
    let after = WorkspaceCrdtDocuments::from_states(&workspace, replica, &reloaded).unwrap();

    assert_eq!(
        after.known_document_ids(),
        before_ids,
        "the same documents must come back, under the same ids"
    );
    // The *session's* authoring clientID is deliberately fresh per construction
    // (a reused stable id is what made merges non-commutative), so it is the
    // restored bytes that must carry the original history — identical bytes mean
    // nothing was re-authored.
    for (document, bytes) in &before_states {
        assert_eq!(
            reloaded.get(document).map(Vec::as_slice),
            Some(bytes.as_ref()),
            "document {document} came back with different bytes"
        );
    }

    cleanup(&workspace_path);
}

/// The migration is two steps that are not atomic together: create the directory,
/// then write it. A device killed in between (quit, crash, power loss) has an
/// empty directory and a legacy blob that is still the only copy of its state.
/// Preferring the empty directory would discard every document.
#[test]
fn a_crash_between_creating_the_directory_and_writing_it_keeps_the_legacy_state() {
    let workspace_path = unique_workspace_path("knotq-upgrade-crash");
    let first = DocumentId::new();
    let second = DocumentId::new();
    write_legacy_blob(
        &workspace_path,
        &HashMap::from([(first, vec![9u8, 8, 7]), (second, vec![1u8, 1])]),
    );
    fs::create_dir_all(crdt_state_dir(&workspace_path)).unwrap();

    let loaded = load_crdt_state(&workspace_path).unwrap();

    assert_eq!(
        loaded.len(),
        2,
        "an empty directory must not discard the legacy state that has not been migrated yet"
    );
    assert_eq!(loaded.get(&first), Some(&vec![9u8, 8, 7]));
    assert_eq!(loaded.get(&second), Some(&vec![1u8, 1]));

    cleanup(&workspace_path);
}

/// Same window, later: some documents are written, the rest are not, and the blob
/// is still in place because retiring it is the last step. The documents that did
/// not make it must come from the blob rather than silently vanish.
#[test]
fn a_half_written_directory_falls_back_to_the_legacy_blob_for_the_missing_documents() {
    let workspace_path = unique_workspace_path("knotq-upgrade-partial");
    let migrated = DocumentId::new();
    let pending = DocumentId::new();
    write_legacy_blob(
        &workspace_path,
        &HashMap::from([(migrated, vec![1u8, 2, 3]), (pending, vec![4u8, 5, 6])]),
    );
    let dir = crdt_state_dir(&workspace_path);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{migrated}.ydoc")), [1u8, 2, 3]).unwrap();

    let loaded = load_crdt_state(&workspace_path).unwrap();

    assert_eq!(loaded.get(&migrated), Some(&vec![1u8, 2, 3]));
    assert_eq!(
        loaded.get(&pending),
        Some(&vec![4u8, 5, 6]),
        "a document not yet migrated must still be read from the legacy blob"
    );

    cleanup(&workspace_path);
}

/// Once the directory is authoritative the blob is gone, so a stale blob can only
/// reappear by an older build being run in between. Its state is older than the
/// directory's by definition, so the directory must win for documents it holds.
#[test]
fn a_reappearing_legacy_blob_never_overwrites_a_migrated_document() {
    let workspace_path = unique_workspace_path("knotq-upgrade-downgrade");
    let document = DocumentId::new();
    save_crdt_state(
        &workspace_path,
        &HashMap::from([(document, b"new".to_vec())]),
    )
    .unwrap();
    write_legacy_blob(
        &workspace_path,
        &HashMap::from([(document, b"old".to_vec())]),
    );

    let loaded = load_crdt_state(&workspace_path).unwrap();

    assert_eq!(
        loaded.get(&document),
        Some(&vec![b'n', b'e', b'w']),
        "the migrated per-document file is the newer state and must win"
    );

    cleanup(&workspace_path);
}

/// Callers take `load_crdt_state(..).unwrap_or_default()`, so an error is not an
/// error to them — it is every document at once. One file this build cannot read
/// must cost that document, not the whole workspace's identity.
#[test]
fn an_unreadable_document_file_does_not_discard_the_rest() {
    let workspace_path = unique_workspace_path("knotq-upgrade-unreadable");
    let healthy = DocumentId::new();
    let damaged = DocumentId::new();
    save_crdt_state(
        &workspace_path,
        &HashMap::from([(healthy, vec![1u8, 2, 3])]),
    )
    .unwrap();

    // A directory where a document file should be: `fs::read` fails on it the
    // way an unreadable or truncated-to-a-device file would, without needing
    // permission games that differ across platforms and CI users.
    let dir = crdt_state_dir(&workspace_path);
    fs::create_dir_all(dir.join(format!("{damaged}.ydoc"))).unwrap();

    let loaded = load_crdt_state(&workspace_path).unwrap();

    assert_eq!(
        loaded.get(&healthy),
        Some(&vec![1u8, 2, 3]),
        "a document that reads fine must survive its neighbour being unreadable"
    );
    assert!(
        !loaded.contains_key(&damaged),
        "the unreadable document must be absent rather than invented"
    );

    cleanup(&workspace_path);
}

/// A settings file written before `upcoming_display` existed must load, with the
/// defaults the panel was shipped with — not fail the parse and reset the user's
/// theme, window and accounts along with it.
#[test]
fn settings_saved_before_the_upcoming_display_keys_load_with_defaults() {
    let workspace_path = unique_workspace_path("knotq-upgrade-settings");
    let settings_path = workspace_path.parent().unwrap().join("settings.json");
    // The shape v0.53.0 wrote: an envelope with no `upcoming_display` key.
    let legacy = serde_json::json!({
        "version": 1,
        "settings": {
            "theme_mode": "dark",
            "time_format": "twenty_four_hour",
            "auto_update": true
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();

    let loaded = load_app_settings(&settings_path).unwrap();

    let defaults = AppSettings::default().upcoming_display;
    assert_eq!(loaded.upcoming_display, defaults);
    assert_eq!(loaded.theme_mode, knotq_model::ThemeMode::Dark);
    assert!(loaded.auto_update);

    cleanup(&workspace_path);
}
