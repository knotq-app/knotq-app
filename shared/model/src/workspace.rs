use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

use crate::{
    daily_queue_scheme_id, daily_queue_sync_metadata, default_folder_sync, default_scheme_sync,
    default_workspace_sync, CrdtBackend, DocumentId, FolderId, Scheme, SchemeId, SyncDocumentKind,
    SyncDocumentMeta, WorkspaceId,
};

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
        };
        workspace.ensure_sync_metadata();
        workspace
    }

    pub fn empty() -> Self {
        Self::new()
    }

    pub fn folder(&self, id: FolderId) -> Option<&Folder> {
        self.folders.get(&id)
    }

    pub fn scheme(&self, id: SchemeId) -> Option<&Scheme> {
        self.schemes.get(&id)
    }

    pub fn scheme_mut(&mut self, id: SchemeId) -> Option<&mut Scheme> {
        self.schemes.get_mut(&id)
    }

    pub fn iter_schemes(&self) -> impl Iterator<Item = &Scheme> {
        self.schemes
            .values()
            .filter(|scheme| !self.is_scheme_deleted(scheme.id))
    }

    pub fn iter_daily_queue_schemes(&self) -> impl Iterator<Item = (NaiveDate, &Scheme)> {
        self.daily_queue.iter().filter_map(|(date, id)| {
            self.schemes
                .get(id)
                .filter(|scheme| !self.is_scheme_deleted(scheme.id))
                .map(|scheme| (*date, scheme))
        })
    }

    pub fn daily_queue_scheme_id(&self, date: NaiveDate) -> Option<SchemeId> {
        self.daily_queue.get(&date).copied()
    }

    pub fn daily_queue_date_for_scheme(&self, scheme_id: SchemeId) -> Option<NaiveDate> {
        self.daily_queue
            .iter()
            .find_map(|(date, id)| (*id == scheme_id).then_some(*date))
    }

    pub fn is_daily_queue_scheme(&self, scheme_id: SchemeId) -> bool {
        self.daily_queue.values().any(|id| *id == scheme_id)
    }

    pub fn iter_deleted_schemes(&self) -> impl Iterator<Item = &Scheme> {
        self.recently_deleted
            .iter()
            .filter_map(|id| self.schemes.get(id))
    }

    pub fn is_scheme_deleted(&self, id: SchemeId) -> bool {
        self.recently_deleted.contains(&id)
    }

    pub fn is_scheme_read_only(&self, id: SchemeId) -> bool {
        self.scheme(id).is_some_and(|s| s.is_read_only())
    }

    pub fn mark_scheme_deleted(&mut self, id: SchemeId) {
        if !self.recently_deleted.contains(&id) {
            self.recently_deleted.push(id);
        }
    }

    pub fn mark_scheme_deleted_at(&mut self, id: SchemeId, position: usize) {
        self.recently_deleted.retain(|deleted| *deleted != id);
        let position = position.min(self.recently_deleted.len());
        self.recently_deleted.insert(position, id);
    }

    pub fn mark_scheme_deleted_from(&mut self, id: SchemeId, folder: FolderId, position: usize) {
        self.mark_scheme_deleted(id);
        self.deleted_scheme_origins
            .insert(id, DeletedSchemeOrigin { folder, position });
    }

    pub fn unmark_scheme_deleted(&mut self, id: SchemeId) {
        self.recently_deleted.retain(|deleted| *deleted != id);
        self.deleted_scheme_origins.remove(&id);
    }

    pub fn deleted_scheme_origin(&self, id: SchemeId) -> Option<DeletedSchemeOrigin> {
        self.deleted_scheme_origins.get(&id).copied()
    }

    /// Walk root -> leaves, returning path from root to the node.
    pub fn path_to(&self, target: NodeRef) -> Vec<FolderId> {
        let mut out = Vec::new();
        self.path_walk(self.root, target, &mut out);
        out
    }

    fn path_walk(&self, current: FolderId, target: NodeRef, out: &mut Vec<FolderId>) -> bool {
        out.push(current);
        if NodeRef::Folder(current) == target {
            return true;
        }
        if let Some(folder) = self.folders.get(&current) {
            for child in &folder.children {
                if *child == target {
                    return true;
                }
                if let NodeRef::Folder(fid) = child {
                    if self.path_walk(*fid, target, out) {
                        return true;
                    }
                }
            }
        }
        out.pop();
        false
    }

    pub fn normalize_one_level_folders(&mut self) -> bool {
        if !self.folders.contains_key(&self.root) {
            return false;
        }
        let mut changed = false;
        let mut visited_folders = HashSet::new();
        let mut referenced_schemes = HashSet::new();
        self.normalize_folder_tree(
            self.root,
            None,
            &mut visited_folders,
            &mut referenced_schemes,
            &mut changed,
        );

        let before_folders = self.folders.len();
        self.folders
            .retain(|id, _| *id == self.root || visited_folders.contains(id));
        if self.folders.len() != before_folders {
            changed = true;
        }

        let daily_queue_ids: HashSet<SchemeId> = self.daily_queue.values().copied().collect();
        let deleted_before = self.recently_deleted.len();
        self.recently_deleted.retain(|id| {
            self.schemes.contains_key(id)
                && !referenced_schemes.contains(id)
                && !daily_queue_ids.contains(id)
        });
        if self.recently_deleted.len() != deleted_before {
            changed = true;
        }
        let deleted_ids: HashSet<SchemeId> = self.recently_deleted.iter().copied().collect();
        let origins_before = self.deleted_scheme_origins.len();
        self.deleted_scheme_origins
            .retain(|id, _| deleted_ids.contains(id));
        if self.deleted_scheme_origins.len() != origins_before {
            changed = true;
        }
        let retained_schemes: HashSet<SchemeId> = referenced_schemes
            .iter()
            .copied()
            .chain(self.recently_deleted.iter().copied())
            .chain(daily_queue_ids.iter().copied())
            .collect();
        let before = self.schemes.len();
        self.schemes.retain(|id, _| retained_schemes.contains(id));
        if self.schemes.len() != before {
            changed = true;
        }

        changed
    }

    fn normalize_folder_tree(
        &mut self,
        folder_id: FolderId,
        expected_parent: Option<FolderId>,
        visited_folders: &mut HashSet<FolderId>,
        referenced_schemes: &mut HashSet<SchemeId>,
        changed: &mut bool,
    ) {
        if !visited_folders.insert(folder_id) {
            *changed = true;
            return;
        }

        let Some(folder) = self.folders.get(&folder_id) else {
            *changed = true;
            return;
        };
        let old_parent = folder.parent;
        let old_children = folder.children.clone();
        if old_parent != expected_parent {
            if let Some(folder) = self.folders.get_mut(&folder_id) {
                folder.parent = expected_parent;
                *changed = true;
            }
        }

        let mut new_children = Vec::with_capacity(old_children.len());
        for child in old_children {
            match child {
                NodeRef::Scheme(id) => {
                    if self.schemes.contains_key(&id)
                        && !self.is_scheme_deleted(id)
                        && !self.is_daily_queue_scheme(id)
                        && referenced_schemes.insert(id)
                    {
                        new_children.push(NodeRef::Scheme(id));
                    } else {
                        *changed = true;
                    }
                }
                NodeRef::Folder(id) => {
                    if id == self.root
                        || !self.folders.contains_key(&id)
                        || visited_folders.contains(&id)
                    {
                        *changed = true;
                        continue;
                    }
                    self.normalize_folder_tree(
                        id,
                        Some(folder_id),
                        visited_folders,
                        referenced_schemes,
                        changed,
                    );
                    new_children.push(NodeRef::Folder(id));
                }
            }
        }

        if self
            .folders
            .get(&folder_id)
            .is_some_and(|folder| folder.children != new_children)
        {
            if let Some(folder) = self.folders.get_mut(&folder_id) {
                folder.children = new_children;
                *changed = true;
            }
        }
    }

    pub fn ensure_sync_metadata(&mut self) -> bool {
        let mut changed = false;
        if self.sync.kind != SyncDocumentKind::PersonalWorkspace {
            self.sync.kind = SyncDocumentKind::PersonalWorkspace;
            changed = true;
        }
        if self.sync.crdt != CrdtBackend::Yrs {
            self.sync.crdt = CrdtBackend::Yrs;
            changed = true;
        }

        let daily_queue_dates_by_id: HashMap<SchemeId, NaiveDate> = self
            .daily_queue
            .iter()
            .map(|(date, id)| (*id, *date))
            .collect();
        let scheme_ids: HashSet<SchemeId> = self.schemes.keys().copied().collect();
        let daily_queue_ids: HashSet<SchemeId> = self.daily_queue.values().copied().collect();
        let sync_scheme_ids: HashSet<SchemeId> = scheme_ids
            .iter()
            .copied()
            .chain(daily_queue_ids.iter().copied())
            .collect();
        let scheme_sync_before = self.scheme_sync.len();
        self.scheme_sync
            .retain(|id, _| sync_scheme_ids.contains(id));
        if self.scheme_sync.len() != scheme_sync_before {
            changed = true;
        }
        for id in sync_scheme_ids {
            let stable_daily_sync = daily_queue_dates_by_id
                .get(&id)
                .copied()
                .filter(|date| daily_queue_scheme_id(*date) == id)
                .map(daily_queue_sync_metadata);
            let entry = self.scheme_sync.entry(id).or_insert_with(|| {
                changed = true;
                stable_daily_sync
                    .clone()
                    .unwrap_or_else(default_scheme_sync)
            });
            if let Some(stable_daily_sync) = stable_daily_sync {
                if entry.id != stable_daily_sync.id {
                    entry.id = stable_daily_sync.id;
                    changed = true;
                }
            }
            if entry.kind != SyncDocumentKind::Scheme {
                entry.kind = SyncDocumentKind::Scheme;
                changed = true;
            }
            if entry.crdt != CrdtBackend::Yrs {
                entry.crdt = CrdtBackend::Yrs;
                changed = true;
            }
        }

        let folder_ids: HashSet<FolderId> = self.folders.keys().copied().collect();
        let folder_sync_before = self.folder_sync.len();
        self.folder_sync.retain(|id, _| folder_ids.contains(id));
        if self.folder_sync.len() != folder_sync_before {
            changed = true;
        }
        for id in folder_ids {
            let entry = self.folder_sync.entry(id).or_insert_with(|| {
                changed = true;
                default_folder_sync()
            });
            if entry.kind != SyncDocumentKind::Folder {
                entry.kind = SyncDocumentKind::Folder;
                changed = true;
            }
            if entry.crdt != CrdtBackend::Yrs {
                entry.crdt = CrdtBackend::Yrs;
                changed = true;
            }
        }

        changed
    }

    pub fn normalize_item_markers(&mut self) -> bool {
        let mut changed = false;
        for scheme in self.schemes.values_mut() {
            for item in &mut scheme.items {
                changed |= item.enforce_marker_constraints();
            }
        }
        changed
    }

    /// Use the account-owned workspace UUID as the stable personal workspace and
    /// workspace-index CRDT document identity. This lets a fresh signed-in device
    /// discover the same workspace document as every other device on the account.
    pub fn canonicalize_personal_sync_identity(&mut self, workspace_id: WorkspaceId) -> bool {
        let mut changed = false;
        if self.id != workspace_id {
            self.id = workspace_id;
            changed = true;
        }
        let document_id = DocumentId(workspace_id.0);
        if self.sync.id != document_id {
            self.sync.id = document_id;
            changed = true;
        }
        changed |=
            self.canonicalize_root_folder_id(personal_workspace_root_folder_id(workspace_id));
        changed |= self.canonicalize_daily_queue_ids();
        changed | self.ensure_sync_metadata()
    }

    fn canonicalize_root_folder_id(&mut self, expected_root: FolderId) -> bool {
        let old_root = self.root;
        let mut changed = false;
        if self.root != expected_root {
            self.root = expected_root;
            changed = true;
        }

        let merge_roots: HashSet<FolderId> = self
            .folders
            .iter()
            .filter_map(|(id, folder)| {
                (*id != expected_root
                    && folder.name == "root"
                    && (folder.parent.is_none()
                        || folder.parent == Some(old_root)
                        || folder.parent == Some(expected_root)))
                .then_some(*id)
            })
            .chain((old_root != expected_root).then_some(old_root))
            .collect();

        let expected_existing = self.folders.remove(&expected_root);
        let mut root_children = expected_existing
            .as_ref()
            .map(|folder| folder.children.clone())
            .unwrap_or_default();
        let mut expanded = expected_existing
            .as_ref()
            .map(|folder| folder.expanded)
            .unwrap_or(true);

        for id in &merge_roots {
            if let Some(folder) = self.folders.remove(id) {
                root_children.extend(folder.children);
                expanded |= folder.expanded;
                changed = true;
            }
        }

        let old_root_ref = NodeRef::Folder(old_root);
        let expected_root_ref = NodeRef::Folder(expected_root);
        let merge_root_refs: HashSet<NodeRef> =
            merge_roots.iter().copied().map(NodeRef::Folder).collect();
        let new_root_children = dedupe_node_refs(root_children.into_iter().filter(|child| {
            *child != old_root_ref
                && *child != expected_root_ref
                && !merge_root_refs.contains(child)
        }));

        let root_needs_insert = expected_existing.as_ref().is_none_or(|folder| {
            folder.id != expected_root
                || folder.name != "root"
                || folder.parent.is_some()
                || folder.children != new_root_children
                || folder.expanded != expanded
        });
        if root_needs_insert {
            changed = true;
        }
        self.folders.insert(
            expected_root,
            Folder {
                id: expected_root,
                name: "root".into(),
                parent: None,
                children: new_root_children,
                expanded,
            },
        );

        for folder in self.folders.values_mut() {
            if folder.id == expected_root {
                if folder.parent.is_some() {
                    folder.parent = None;
                    changed = true;
                }
            } else if folder.parent == Some(old_root)
                || folder
                    .parent
                    .is_some_and(|parent| merge_roots.contains(&parent))
            {
                folder.parent = Some(expected_root);
                changed = true;
            }

            let old_children = folder.children.clone();
            let new_children = dedupe_node_refs(old_children.into_iter().flat_map(|child| {
                if child == expected_root_ref
                    || child == old_root_ref
                    || merge_root_refs.contains(&child)
                {
                    Vec::new()
                } else {
                    vec![child]
                }
            }));
            if folder.children != new_children {
                folder.children = new_children;
                changed = true;
            }
        }

        if old_root != expected_root && self.folder_sync.remove(&old_root).is_some() {
            changed = true;
        }
        for id in &merge_roots {
            if self.folder_sync.remove(id).is_some() {
                changed = true;
            }
        }
        for origin in self.deleted_scheme_origins.values_mut() {
            if origin.folder == old_root || merge_roots.contains(&origin.folder) {
                origin.folder = expected_root;
                changed = true;
            }
        }
        changed
    }

    fn canonicalize_daily_queue_ids(&mut self) -> bool {
        let inferred_daily_queues = self
            .schemes
            .iter()
            .filter_map(|(id, scheme)| {
                daily_queue_date_from_scheme_name(&scheme.name)
                    .filter(|date| !self.daily_queue.contains_key(date))
                    .map(|date| (date, *id))
            })
            .collect::<Vec<_>>();
        let mut changed = false;
        for (date, id) in inferred_daily_queues {
            self.daily_queue.insert(date, id);
            changed = true;
        }

        let entries = self.daily_queue.clone();
        let mut daily_ids = HashSet::new();

        for (date, current_id) in entries {
            let expected_id = daily_queue_scheme_id(date);
            daily_ids.insert(expected_id);
            if current_id == expected_id {
                continue;
            }

            let mut replacement = self.schemes.remove(&expected_id);
            if let Some(mut legacy) = self.schemes.remove(&current_id) {
                legacy.id = expected_id;
                match &mut replacement {
                    Some(existing) => merge_daily_queue_scheme(existing, legacy),
                    None => replacement = Some(legacy),
                }
            }
            if let Some(mut scheme) = replacement {
                scheme.id = expected_id;
                self.schemes.insert(expected_id, scheme);
            }

            self.daily_queue.insert(date, expected_id);
            self.scheme_sync.remove(&current_id);
            self.deleted_scheme_origins.remove(&current_id);
            self.recently_deleted
                .retain(|id| *id != current_id && *id != expected_id);
            daily_ids.insert(current_id);
            changed = true;
        }

        for (date, id) in &self.daily_queue {
            let expected_id = daily_queue_scheme_id(*date);
            if *id == expected_id {
                let sync = daily_queue_sync_metadata(*date);
                if self.scheme_sync.get(&expected_id) != Some(&sync) {
                    self.scheme_sync.insert(expected_id, sync);
                    changed = true;
                }
                daily_ids.insert(expected_id);
            }
        }

        for id in &daily_ids {
            if self.deleted_scheme_origins.remove(id).is_some() {
                changed = true;
            }
        }
        let deleted_before = self.recently_deleted.len();
        self.recently_deleted.retain(|id| !daily_ids.contains(id));
        if self.recently_deleted.len() != deleted_before {
            changed = true;
        }

        for folder in self.folders.values_mut() {
            let before = folder.children.len();
            folder
                .children
                .retain(|child| !matches!(child, NodeRef::Scheme(id) if daily_ids.contains(id)));
            if folder.children.len() != before {
                changed = true;
            }
        }

        changed
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

fn dedupe_node_refs(children: impl IntoIterator<Item = NodeRef>) -> Vec<NodeRef> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for child in children {
        if seen.insert(child) {
            out.push(child);
        }
    }
    out
}

fn merge_daily_queue_scheme(existing: &mut Scheme, legacy: Scheme) {
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

fn daily_queue_date_from_scheme_name(name: &str) -> Option<NaiveDate> {
    name.strip_prefix("Daily ")
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Item, Scheme, DAILY_QUEUE_COLOR_INDEX};

    #[test]
    fn normalize_removes_unreferenced_schemes_unless_recently_deleted() {
        let mut workspace = Workspace::new();
        let referenced = Scheme::new("Shown", 0);
        let referenced_id = referenced.id;
        let orphan = Scheme::new("Deleted", 1);
        let orphan_id = orphan.id;
        workspace.schemes.insert(referenced_id, referenced);
        workspace.schemes.insert(orphan_id, orphan);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(referenced_id));

        assert!(workspace.normalize_one_level_folders());
        assert!(workspace.schemes.contains_key(&referenced_id));
        assert!(!workspace.schemes.contains_key(&orphan_id));

        let mut workspace = Workspace::new();
        let deleted = Scheme::new("Deleted", 1);
        let deleted_id = deleted.id;
        workspace.schemes.insert(deleted_id, deleted);
        workspace.mark_scheme_deleted(deleted_id);

        assert!(!workspace.normalize_one_level_folders());
        assert!(workspace.schemes.contains_key(&deleted_id));
        assert!(workspace.is_scheme_deleted(deleted_id));
    }

    #[test]
    fn normalize_preserves_nested_folders() {
        let mut workspace = Workspace::new();
        let child = FolderId::new();
        let grandchild = FolderId::new();
        let scheme = Scheme::new("Nested", 0);
        let scheme_id = scheme.id;

        workspace.folders.insert(
            child,
            Folder {
                id: child,
                name: "Child".into(),
                parent: Some(workspace.root),
                children: vec![NodeRef::Folder(grandchild)],
                expanded: true,
            },
        );
        workspace.folders.insert(
            grandchild,
            Folder {
                id: grandchild,
                name: "Grandchild".into(),
                parent: Some(child),
                children: vec![NodeRef::Scheme(scheme_id)],
                expanded: true,
            },
        );
        workspace.schemes.insert(scheme_id, scheme);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(child));

        assert!(!workspace.normalize_one_level_folders());
        assert_eq!(
            workspace.folder(child).unwrap().children,
            vec![NodeRef::Folder(grandchild)]
        );
        assert_eq!(workspace.folder(grandchild).unwrap().parent, Some(child));
        assert!(workspace.schemes.contains_key(&scheme_id));
    }

    #[test]
    fn normalize_keeps_trash_and_daily_queue_out_of_sidebar() {
        let mut workspace = Workspace::new();
        let active = Scheme::new("Active", 0);
        let active_id = active.id;
        let deleted = Scheme::new("Archived", 1);
        let deleted_id = deleted.id;
        let daily_date = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let daily_id = daily_queue_scheme_id(daily_date);
        let mut daily = Scheme::new("Daily", 0);
        daily.id = daily_id;

        workspace.schemes.insert(active_id, active);
        workspace.schemes.insert(deleted_id, deleted);
        workspace.schemes.insert(daily_id, daily);
        workspace.mark_scheme_deleted_from(deleted_id, workspace.root, 1);
        workspace.daily_queue.insert(daily_date, daily_id);
        workspace.folders.get_mut(&workspace.root).unwrap().children = vec![
            NodeRef::Scheme(active_id),
            NodeRef::Scheme(deleted_id),
            NodeRef::Scheme(daily_id),
        ];

        assert!(workspace.normalize_one_level_folders());
        assert_eq!(
            workspace.folder(workspace.root).unwrap().children,
            vec![NodeRef::Scheme(active_id)]
        );
        assert!(workspace.is_scheme_deleted(deleted_id));
        assert_eq!(workspace.daily_queue_scheme_id(daily_date), Some(daily_id));
        assert!(workspace.schemes.contains_key(&deleted_id));
        assert!(workspace.schemes.contains_key(&daily_id));
    }

    #[test]
    fn canonical_personal_sync_identity_uses_account_workspace_id() {
        let mut workspace = Workspace::new();
        let account_workspace = WorkspaceId::new();
        let old_root = workspace.root;
        let expected_root = personal_workspace_root_folder_id(account_workspace);

        assert!(workspace.canonicalize_personal_sync_identity(account_workspace));
        assert_eq!(workspace.id, account_workspace);
        assert_eq!(workspace.sync.id, DocumentId(account_workspace.0));
        assert_eq!(workspace.sync.kind, SyncDocumentKind::PersonalWorkspace);
        assert_eq!(workspace.root, expected_root);
        assert!(workspace.folders.contains_key(&expected_root));
        assert!(!workspace.folders.contains_key(&old_root));

        assert!(!workspace.canonicalize_personal_sync_identity(account_workspace));
    }

    #[test]
    fn canonical_personal_sync_identity_merges_duplicate_roots() {
        let account_workspace = WorkspaceId::new();
        let mut workspace = Workspace::new();
        let old_root = workspace.root;
        let local = Scheme::new("Local", 0);
        let local_id = local.id;
        let remote = Scheme::new("Remote", 1);
        let remote_id = remote.id;
        let duplicate_root = FolderId::new();

        workspace.schemes.insert(local_id, local);
        workspace.schemes.insert(remote_id, remote);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .extend([NodeRef::Scheme(local_id), NodeRef::Folder(duplicate_root)]);
        workspace.folders.insert(
            duplicate_root,
            Folder {
                id: duplicate_root,
                name: "root".into(),
                parent: None,
                children: vec![NodeRef::Scheme(remote_id)],
                expanded: true,
            },
        );

        assert!(workspace.canonicalize_personal_sync_identity(account_workspace));
        let expected_root = personal_workspace_root_folder_id(account_workspace);
        let root_children = &workspace.folder(expected_root).unwrap().children;
        assert_eq!(workspace.root, expected_root);
        assert!(root_children.contains(&NodeRef::Scheme(local_id)));
        assert!(root_children.contains(&NodeRef::Scheme(remote_id)));
        assert!(!root_children.contains(&NodeRef::Folder(old_root)));
        assert!(!root_children.contains(&NodeRef::Folder(duplicate_root)));
        assert!(!workspace.folders.contains_key(&old_root));
        assert!(!workspace.folders.contains_key(&duplicate_root));
    }

    #[test]
    fn canonical_personal_sync_identity_migrates_legacy_daily_queue_ids() {
        let date = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let mut workspace = Workspace::new();
        let workspace_id = workspace.id;
        let legacy_id = SchemeId::new();
        let expected_id = daily_queue_scheme_id(date);
        let mut legacy = Scheme::new("Daily 2026-05-31", DAILY_QUEUE_COLOR_INDEX);
        legacy.id = legacy_id;
        legacy.items.push(Item::new("legacy entry"));

        workspace.schemes.insert(legacy_id, legacy);
        workspace.daily_queue.insert(date, legacy_id);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(legacy_id));

        assert!(workspace.canonicalize_personal_sync_identity(workspace_id));
        assert_eq!(workspace.daily_queue_scheme_id(date), Some(expected_id));
        assert!(!workspace.schemes.contains_key(&legacy_id));
        assert_eq!(
            workspace.schemes[&expected_id].items[0].text,
            "legacy entry"
        );
        assert_eq!(
            workspace.scheme_sync[&expected_id].id,
            crate::daily_queue_document_id(date)
        );
        assert!(!workspace
            .folder(workspace.root)
            .unwrap()
            .children
            .contains(&NodeRef::Scheme(legacy_id)));
        assert!(!workspace
            .folder(workspace.root)
            .unwrap()
            .children
            .contains(&NodeRef::Scheme(expected_id)));
    }

    #[test]
    fn canonical_personal_sync_identity_infers_visible_daily_queue_schemes() {
        let date = NaiveDate::from_ymd_opt(2026, 5, 26).unwrap();
        let mut workspace = Workspace::new();
        let workspace_id = workspace.id;
        let legacy_id = SchemeId::new();
        let expected_id = daily_queue_scheme_id(date);
        let mut legacy = Scheme::new("Daily 2026-05-26", DAILY_QUEUE_COLOR_INDEX);
        legacy.id = legacy_id;

        workspace.schemes.insert(legacy_id, legacy);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(legacy_id));

        assert!(workspace.canonicalize_personal_sync_identity(workspace_id));
        assert_eq!(workspace.daily_queue_scheme_id(date), Some(expected_id));
        assert!(workspace.schemes.contains_key(&expected_id));
        assert!(!workspace
            .folder(workspace.root)
            .unwrap()
            .children
            .contains(&NodeRef::Scheme(legacy_id)));
    }
}
