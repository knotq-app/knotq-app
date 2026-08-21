//! The registered migrations, oldest first.
//!
//! Append only. The ids are recorded in users' `data-layout.json`, so renaming
//! one makes every existing install believe it has an unapplied migration.

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context};

use super::Migration;
use crate::crdt_state::{
    crdt_state_dir, crdt_state_path, document_file_name, load_from_dir, load_single_blob,
    retire_single_blob, retired_crdt_state_path,
};

pub static ALL: &[Migration] = &[CRDT_STATE_PER_DOCUMENT_FILES];

/// `sync-crdt-state.json` (one JSON blob of base64 documents) →
/// `sync-crdt-state/<document>.ydoc` (raw bytes, one file each).
///
/// Shipped after v0.53.0. It ran implicitly, inside the first save, which is
/// exactly the failure this module exists to prevent: a device killed between
/// creating the directory and filling it came back with neither form complete,
/// and a document that comes back absent is rebuilt empty under a fresh Yjs
/// identity — the account re-seeds itself from nothing. Running it here means it
/// happens once, before anything else touches the data, with the old blob backed
/// up and the result verified before the blob is retired.
const CRDT_STATE_PER_DOCUMENT_FILES: Migration = Migration {
    id: "crdt-state-per-document-files",
    summary: "split sync-crdt-state.json into one file per document",
    paths: |paths| -> Vec<PathBuf> {
        vec![
            crdt_state_path(&paths.workspace_path),
            crdt_state_dir(&paths.workspace_path),
            retired_crdt_state_path(&paths.workspace_path),
        ]
    },
    is_pending: |paths| {
        // The blob still under its live name is the signal: it is renamed to
        // `.migrated` only once the directory holds everything.
        let blob = crdt_state_path(&paths.workspace_path);
        Ok(blob.exists()
            && fs::metadata(&blob)
                .map(|meta| meta.len() > 0)
                .unwrap_or(false))
    },
    apply: |paths| {
        let blob = load_single_blob(&crdt_state_path(&paths.workspace_path))?;
        let dir = crdt_state_dir(&paths.workspace_path);
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

        // Only documents the directory does not already hold. A file that is
        // there is either from a previous attempt at this same migration or
        // newer than the blob, and in both cases it is the state to keep.
        let existing = load_from_dir(&dir)?;
        for (document, bytes) in &blob {
            if existing.contains_key(document) {
                continue;
            }
            crate::files::write_atomic(&dir.join(document_file_name(*document)), bytes)?;
        }
        retire_single_blob(&paths.workspace_path);
        Ok(())
    },
    verify: |paths| {
        // Read the new form back off disk and prove it carries every document
        // the old one did. Anything less and the blob would be retired on the
        // strength of writes we never confirmed.
        let retired = load_single_blob(&retired_crdt_state_path(&paths.workspace_path))?;
        let migrated = load_from_dir(&crdt_state_dir(&paths.workspace_path))?;
        for (document, bytes) in &retired {
            match migrated.get(document) {
                Some(written) if written == bytes => {}
                // Present but different: a newer per-document file already held
                // this document, which `apply` deliberately left alone.
                Some(_) => {}
                None => {
                    return Err(anyhow!(
                        "document {document} is missing from the per-document state directory"
                    ))
                }
            }
        }
        if crdt_state_path(&paths.workspace_path).exists() {
            return Err(anyhow!("the single-blob state file was not retired"));
        }
        Ok(())
    },
};
