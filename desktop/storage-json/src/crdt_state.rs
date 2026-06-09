//! Durable persistence for the long-lived CRDT documents' `state_v1` bytes.
//!
//! The CRDT documents are never rebuilt from plain data with a throwaway identity;
//! they are restored from this file (via [`knotq_sync::WorkspaceCrdtDocuments::from_states`])
//! with a deterministic clientID, so their Yjs identity survives app restarts and
//! the desktop UI↔background-thread split. The file lives next to `sync-state.json`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use knotq_model::DocumentId;
use knotq_sync::{PersistedCrdtState, LOCAL_CRDT_STATE_FILE};

use crate::sync_state::sync_state_data_dir;

pub fn crdt_state_path(workspace_path: &Path) -> PathBuf {
    sync_state_data_dir(workspace_path).join(LOCAL_CRDT_STATE_FILE)
}

pub fn load_crdt_state(workspace_path: &Path) -> Result<HashMap<DocumentId, Vec<u8>>> {
    let path = crdt_state_path(workspace_path);
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(HashMap::new());
    }
    let persisted: PersistedCrdtState =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(persisted.into_states())
}

pub fn save_crdt_state(workspace_path: &Path, states: &HashMap<DocumentId, Vec<u8>>) -> Result<()> {
    let path = crdt_state_path(workspace_path);
    let persisted = PersistedCrdtState::from_states(states);
    let json = serde_json::to_string(&persisted).context("serialize CRDT state")?;
    crate::files::write_atomic(&path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn crdt_state_round_trips_through_file() {
        let dir = std::env::temp_dir().join(format!("knotq-crdt-state-test-{}", Uuid::new_v4()));
        let workspace_path = dir.join("workspace").join("workspace.json");
        let mut states = HashMap::new();
        let doc = DocumentId::new();
        states.insert(doc, vec![1u8, 2, 3, 255]);

        save_crdt_state(&workspace_path, &states).unwrap();
        let loaded = load_crdt_state(&workspace_path).unwrap();

        assert_eq!(
            crdt_state_path(&workspace_path),
            dir.join("sync-crdt-state.json")
        );
        assert_eq!(loaded.get(&doc), Some(&vec![1u8, 2, 3, 255]));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = std::env::temp_dir().join(format!("knotq-crdt-state-missing-{}", Uuid::new_v4()));
        let workspace_path = dir.join("workspace").join("workspace.json");
        assert!(load_crdt_state(&workspace_path).unwrap().is_empty());
    }
}
