use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use knotq_model::{ItemId, OperationId, Workspace};
use knotq_sync::{
    LocalSyncState, PendingCrdtEdit, QueuedItemFields, RecentFolderEdit, RecentItemEdit,
    LOCAL_SYNC_STATE_FILE,
};
use std::collections::HashMap;

pub fn sync_state_data_dir(workspace_path: &Path) -> PathBuf {
    let workspace_dir = workspace_path.parent().unwrap_or_else(|| Path::new("."));
    if workspace_dir
        .file_name()
        .is_some_and(|name| name == "workspace")
    {
        return workspace_dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
    }
    workspace_dir.to_path_buf()
}

pub fn sync_state_path(workspace_path: &Path) -> PathBuf {
    sync_state_data_dir(workspace_path).join(LOCAL_SYNC_STATE_FILE)
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
    match serde_json::from_str(&raw) {
        Ok(state) => Ok(state),
        Err(error) => {
            // A damaged sync-state file must never be silently overwritten by a
            // default state. It may be the only record of an unsent edit. Keep a
            // recoverable copy, then let the caller start from clean cursors so
            // the next sync re-pulls the server's merged state.
            let backup = unreadable_backup_path(&path);
            fs::rename(&path, &backup).or_else(|rename_error| {
                fs::copy(&path, &backup).map(|_| ()).with_context(|| {
                    format!("preserve unreadable sync state after rename failed ({rename_error})")
                })
            })?;
            eprintln!(
                "sync state parse failed; preserved {} as {}: {error}",
                path.display(),
                backup.display()
            );
            Ok(LocalSyncState::default())
        }
    }
}

fn unreadable_backup_path(path: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    path.with_file_name(format!(
        "{}.unreadable-{stamp}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("sync-state")
    ))
}

pub fn save_local_sync_state(workspace_path: &Path, state: &LocalSyncState) -> Result<()> {
    let path = sync_state_path(workspace_path);
    let json = serde_json::to_string_pretty(state).context("serialize local sync state")?;
    crate::files::write_atomic(&path, json.as_bytes())
}

/// Record the plain workspace that is on disk before a paired workspace/CRDT
/// save starts. The marker lives beside the sync cursors so it is written
/// atomically with the rest of the durable checkpoint. It is intentionally
/// kept until the CRDT state has been written too: a process can die after the
/// workspace file is replaced but before the queue or CRDT files are updated.
pub fn begin_workspace_save_recovery(workspace_path: &Path, workspace: &Workspace) -> Result<()> {
    let mut state = load_local_sync_state(workspace_path)?;
    state.workspace_save_recovery =
        Some(serde_json::to_string(workspace).context("serialize workspace save recovery base")?);
    save_local_sync_state(workspace_path, &state)
}

/// Load the pre-save workspace captured by a paired-save recovery marker.
pub fn load_workspace_save_recovery(workspace_path: &Path) -> Result<Option<Workspace>> {
    let state = load_local_sync_state(workspace_path)?;
    state
        .workspace_save_recovery
        .as_deref()
        .map(|raw| serde_json::from_str(raw).context("parse workspace save recovery base"))
        .transpose()
}

/// Clear a completed paired workspace/CRDT save's recovery marker.
pub fn clear_workspace_save_recovery(workspace_path: &Path) -> Result<()> {
    let mut state = load_local_sync_state(workspace_path)?;
    if state.workspace_save_recovery.take().is_some() {
        save_local_sync_state(workspace_path, &state)?;
    }
    Ok(())
}

pub fn save_pending_crdt_edits(workspace_path: &Path, pending: &[PendingCrdtEdit]) -> Result<()> {
    save_pending_crdt_edits_with_item_fields(workspace_path, pending, &Default::default())
}

/// [`save_pending_crdt_edits`], also recording which line fields each queued
/// edit changes (see `LocalSyncState::queued_item_fields`).
pub fn save_pending_crdt_edits_with_item_fields(
    workspace_path: &Path,
    pending: &[PendingCrdtEdit],
    item_fields: &std::collections::HashMap<OperationId, Vec<QueuedItemFields>>,
) -> Result<()> {
    let mut state = load_local_sync_state(workspace_path)?;
    for (operation, fields) in item_fields {
        if pending.iter().any(|edit| edit.operation_id == *operation) {
            state.record_queued_item_fields(*operation, fields.clone());
        }
    }
    for edit in pending {
        if !state.pending.iter().any(|existing| {
            existing.operation_id == edit.operation_id
                && existing.document == edit.document
                && existing.local_sequence == edit.local_sequence
        }) {
            state.push_pending(edit.clone());
        }
    }
    state.prune_queued_item_fields();
    save_local_sync_state(workspace_path, &state)
}

/// Replace the durable pending queue with the live store snapshot.
///
/// A normal background sync owns its complete `LocalSyncState` and writes it
/// directly. The UI save path only has the live pending edits, though, so the
/// old append-only helper could resurrect an edit that the UI had already
/// acknowledged and cleared. That stale edit then got pushed forever on every
/// subsequent sync. Metadata owned by the sync engine (cursors, account
/// identity, recovery flags) remains intact; only the queue is reconciled.
pub fn replace_pending_crdt_edits_with_item_fields(
    workspace_path: &Path,
    pending: &[PendingCrdtEdit],
    item_fields: &std::collections::HashMap<OperationId, Vec<QueuedItemFields>>,
) -> Result<()> {
    let mut state = load_local_sync_state(workspace_path)?;
    state.pending = pending.iter().cloned().collect();
    for (operation, fields) in item_fields {
        if pending.iter().any(|edit| edit.operation_id == *operation) {
            state.record_queued_item_fields(*operation, fields.clone());
        }
    }
    state.prune_queued_item_fields();
    save_local_sync_state(workspace_path, &state)
}

/// Merge the in-memory acknowledged item journal into the durable sync state.
/// A save racing a sync must not erase provenance needed by a later moved-item
/// repair.
pub fn merge_recent_item_edits(
    workspace_path: &Path,
    records: &HashMap<ItemId, RecentItemEdit>,
) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut state = load_local_sync_state(workspace_path)?;
    state
        .recent_item_edits
        .extend(records.iter().map(|(item, record)| (*item, record.clone())));
    save_local_sync_state(workspace_path, &state)
}

/// Merge acknowledged folder-index provenance into the durable sync state.
pub fn merge_recent_folder_edits(
    workspace_path: &Path,
    records: &HashMap<knotq_model::FolderId, RecentFolderEdit>,
) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut state = load_local_sync_state(workspace_path)?;
    state.recent_folder_edits.extend(
        records
            .iter()
            .map(|(folder, record)| (*folder, record.clone())),
    );
    save_local_sync_state(workspace_path, &state)
}

/// Replace the acknowledged item journal during the shutdown flush. Unlike a
/// background save, shutdown has the complete in-memory state and must be able
/// to retire entries that are no longer safe to replay after a restart.
pub fn replace_recent_item_edits(
    workspace_path: &Path,
    records: &HashMap<ItemId, RecentItemEdit>,
) -> Result<()> {
    let mut state = load_local_sync_state(workspace_path)?;
    state.recent_item_edits = records.clone();
    save_local_sync_state(workspace_path, &state)
}

/// Replace the acknowledged folder-index journal during shutdown.
pub fn replace_recent_folder_edits(
    workspace_path: &Path,
    records: &HashMap<knotq_model::FolderId, RecentFolderEdit>,
) -> Result<()> {
    let mut state = load_local_sync_state(workspace_path)?;
    state.recent_folder_edits = records.clone();
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
        let workspace_path = dir.join("workspace").join("workspace.json");
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
            touched_items: Vec::new(),
        }];

        save_pending_crdt_edits(&workspace_path, &pending).unwrap();
        let loaded = load_local_sync_state(&workspace_path).unwrap();

        assert_eq!(
            sync_state_path(&workspace_path),
            dir.join("sync-state.json")
        );
        assert_eq!(loaded.pending.len(), 1);
        assert_eq!(loaded.pending[0].document, document);
        assert_eq!(loaded.pending[0].update_v1, vec![1, 2, 3]);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_sync_state_is_preserved_before_recovery() {
        let dir = std::env::temp_dir().join(format!("knotq-sync-state-corrupt-{}", Uuid::new_v4()));
        let workspace_path = dir.join("workspace").join("workspace.json");
        let path = sync_state_path(&workspace_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{not valid json").unwrap();

        let recovered = load_local_sync_state(&workspace_path).unwrap();

        assert_eq!(recovered, LocalSyncState::default());
        assert!(
            !path.exists(),
            "the corrupt source must not remain in the write path"
        );
        let backups = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("sync-state.json.unreadable-"))
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(dir.join(&backups[0])).unwrap(), b"{not valid json");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn workspace_save_recovery_round_trips_and_clears() {
        let dir =
            std::env::temp_dir().join(format!("knotq-workspace-save-recovery-{}", Uuid::new_v4()));
        let workspace_path = dir.join("workspace").join("workspace.json");
        let base = Workspace::new();

        begin_workspace_save_recovery(&workspace_path, &base).unwrap();
        assert_eq!(
            load_workspace_save_recovery(&workspace_path).unwrap(),
            Some(base)
        );

        clear_workspace_save_recovery(&workspace_path).unwrap();
        assert_eq!(load_workspace_save_recovery(&workspace_path).unwrap(), None);
        let _ = fs::remove_dir_all(dir);
    }
}
