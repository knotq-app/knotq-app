use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use knotq_sync::{LocalSyncState, PendingCrdtEdit, LOCAL_SYNC_DIR, LOCAL_SYNC_STATE_FILE};

pub fn sync_state_dir(workspace_path: &Path) -> PathBuf {
    workspace_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LOCAL_SYNC_DIR)
}

pub fn sync_state_path(workspace_path: &Path) -> PathBuf {
    sync_state_dir(workspace_path).join(LOCAL_SYNC_STATE_FILE)
}

pub fn load_local_sync_state(workspace_path: &Path) -> Result<LocalSyncState> {
    let path = sync_state_path(workspace_path);
    if !path.exists() {
        return Ok(LocalSyncState::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(LocalSyncState::default());
    }
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

pub fn save_local_sync_state(workspace_path: &Path, state: &LocalSyncState) -> Result<()> {
    let path = sync_state_path(workspace_path);
    let json = serde_json::to_string_pretty(state).context("serialize local sync state")?;
    crate::files::write_atomic(&path, json.as_bytes())
}

pub fn save_pending_crdt_edits(workspace_path: &Path, pending: &[PendingCrdtEdit]) -> Result<()> {
    let mut state = load_local_sync_state(workspace_path)?;
    state.replace_pending(pending.iter().cloned());
    save_local_sync_state(workspace_path, &state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, WorkspaceId};
    use uuid::Uuid;

    #[test]
    fn pending_crdt_edits_round_trip_through_sync_state_file() {
        let dir = std::env::temp_dir().join(format!("knotq-sync-state-test-{}", Uuid::new_v4()));
        let workspace_path = dir.join("workspace.json");
        let workspace_id = WorkspaceId::new();
        let replica_id = ReplicaId::new();
        let document = DocumentId::new();
        let pending = vec![PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id,
            replica_id,
            local_sequence: 7,
            created_at: Utc::now(),
            document,
            kind: SyncDocumentKind::Scheme,
            update_v1: vec![1, 2, 3],
        }];

        save_pending_crdt_edits(&workspace_path, &pending).unwrap();
        let loaded = load_local_sync_state(&workspace_path).unwrap();

        assert_eq!(loaded.pending.len(), 1);
        assert_eq!(loaded.pending[0].document, document);
        assert_eq!(loaded.pending[0].update_v1, vec![1, 2, 3]);

        let _ = fs::remove_dir_all(dir);
    }
}
