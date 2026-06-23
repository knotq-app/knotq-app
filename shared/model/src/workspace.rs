use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

use crate::{
    default_workspace_sync, FolderId, Scheme, SchemeId, SyncDocumentMeta, WorkspaceId,
};

mod archive;
mod lookup;
mod normalize;
mod sync_identity;

const PERSONAL_WORKSPACE_ROOT_FOLDER_NAMESPACE: [u8; 16] = [
    0xd8, 0x7b, 0xce, 0x73, 0x80, 0x0d, 0x4b, 0x27, 0x93, 0x62, 0x66, 0x15, 0x17, 0xe2, 0x8e, 0xd4,
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum NodeRef {
    Folder(FolderId),
    Scheme(SchemeId),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub id: WorkspaceId,
    #[serde(default = "default_workspace_sync")]
    pub sync: SyncDocumentMeta,
    pub root: FolderId,
    pub folders: HashMap<FolderId, Folder>,
    pub schemes: HashMap<SchemeId, Scheme>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scheme_sync: HashMap<SchemeId, SyncDocumentMeta>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub folder_sync: HashMap<FolderId, SyncDocumentMeta>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub daily_queue: BTreeMap<NaiveDate, SchemeId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recently_deleted: Vec<SchemeId>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub deleted_scheme_origins: HashMap<SchemeId, DeletedSchemeOrigin>,
    /// Top-level archived folders, newest-first like [`recently_deleted`]. The folder
    /// (and its whole subtree) stays in [`folders`]/[`schemes`] — detached from
    /// [`root`] — so the archive can show it as a folder and restore it as one unit.
    /// Each scheme inside an archived folder subtree is also recorded in
    /// [`recently_deleted`], so the existing per-scheme "is archived" checks
    /// (calendar, sidebar, search) keep working unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recently_deleted_folders: Vec<FolderId>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub deleted_folder_origins: HashMap<FolderId, DeletedFolderOrigin>,
}

impl Workspace {
    pub fn new() -> Self {
        let id = WorkspaceId::new();
        let root = personal_workspace_root_folder_id(id);
        let mut folders = HashMap::new();
        folders.insert(
            root,
            Folder {
                id: root,
                name: "root".into(),
                parent: None,
                children: Vec::new(),
                expanded: true,
            },
        );
        let mut workspace = Self {
            id,
            sync: default_workspace_sync(),
            root,
            folders,
            schemes: HashMap::new(),
            scheme_sync: HashMap::new(),
            folder_sync: HashMap::new(),
            daily_queue: BTreeMap::new(),
            recently_deleted: Vec::new(),
            deleted_scheme_origins: HashMap::new(),
            recently_deleted_folders: Vec::new(),
            deleted_folder_origins: HashMap::new(),
        };
        workspace.ensure_sync_metadata();
        workspace
    }

    pub fn empty() -> Self {
        Self::new()
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<FolderId>,
    pub children: Vec<NodeRef>,
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeletedSchemeOrigin {
    pub folder: FolderId,
    pub position: usize,
}

/// Where an archived folder lived before deletion, so it restores to the same place.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeletedFolderOrigin {
    pub parent: FolderId,
    pub position: usize,
}

pub fn personal_workspace_root_folder_id(workspace_id: WorkspaceId) -> FolderId {
    FolderId(stable_workspace_uuid(
        PERSONAL_WORKSPACE_ROOT_FOLDER_NAMESPACE,
        &workspace_id.to_string(),
    ))
}

fn stable_workspace_uuid(namespace: [u8; 16], name: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(namespace);
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

pub(super) fn dedupe_node_refs(children: impl IntoIterator<Item = NodeRef>) -> Vec<NodeRef> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for child in children {
        if seen.insert(child) {
            out.push(child);
        }
    }
    out
}

pub(super) fn merge_daily_queue_scheme(existing: &mut Scheme, legacy: Scheme) {
    let mut item_ids = existing
        .items
        .iter()
        .map(|item| item.id)
        .collect::<HashSet<_>>();
    for item in legacy.items {
        if item_ids.insert(item.id) {
            existing.items.push(item);
        }
    }
    if existing.name.is_empty() {
        existing.name = legacy.name;
    }
    existing.color_index = crate::DAILY_QUEUE_COLOR_INDEX;
}

pub(super) fn daily_queue_date_from_scheme_name(name: &str) -> Option<NaiveDate> {
    name.strip_prefix("Daily ")
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
}
