use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{
    default_folder_sync, default_scheme_sync, default_workspace_sync, CrdtBackend, FolderId,
    Scheme, SchemeId, SyncDocumentKind, SyncDocumentMeta, WorkspaceId,
};

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
        let root = FolderId::new();
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
        let Some(root_folder) = self.folders.get(&self.root) else {
            return false;
        };
        let root_children = root_folder.children.clone();
        let mut changed = false;
        let mut new_root_children = Vec::with_capacity(root_children.len());
        let mut folders_to_remove = Vec::new();

        for child in root_children {
            match child {
                NodeRef::Scheme(id) => {
                    if self.schemes.contains_key(&id) {
                        new_root_children.push(NodeRef::Scheme(id));
                    } else {
                        changed = true;
                    }
                }
                NodeRef::Folder(id) => {
                    if id == self.root || !self.folders.contains_key(&id) {
                        changed = true;
                        continue;
                    }

                    let mut direct_children = Vec::new();
                    self.collect_schemes_for_one_level_folder(
                        id,
                        &mut direct_children,
                        &mut folders_to_remove,
                        &mut changed,
                    );

                    if let Some(folder) = self.folders.get_mut(&id) {
                        if folder.parent != Some(self.root) {
                            folder.parent = Some(self.root);
                            changed = true;
                        }
                        if folder.children != direct_children {
                            folder.children = direct_children;
                            changed = true;
                        }
                    }
                    new_root_children.push(NodeRef::Folder(id));
                }
            }
        }

        if let Some(root_folder) = self.folders.get_mut(&self.root) {
            if root_folder.children != new_root_children {
                root_folder.children = new_root_children;
                changed = true;
            }
        }

        for id in folders_to_remove {
            if id != self.root && self.folders.remove(&id).is_some() {
                changed = true;
            }
        }

        let referenced_schemes = self.referenced_scheme_ids();
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

        let scheme_ids: HashSet<SchemeId> = self.schemes.keys().copied().collect();
        let scheme_sync_before = self.scheme_sync.len();
        self.scheme_sync.retain(|id, _| scheme_ids.contains(id));
        if self.scheme_sync.len() != scheme_sync_before {
            changed = true;
        }
        for id in scheme_ids {
            let entry = self.scheme_sync.entry(id).or_insert_with(|| {
                changed = true;
                default_scheme_sync()
            });
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

    fn collect_schemes_for_one_level_folder(
        &self,
        folder_id: FolderId,
        out: &mut Vec<NodeRef>,
        folders_to_remove: &mut Vec<FolderId>,
        changed: &mut bool,
    ) {
        let Some(folder) = self.folders.get(&folder_id) else {
            *changed = true;
            return;
        };

        for child in folder.children.clone() {
            match child {
                NodeRef::Scheme(id) => {
                    if self.schemes.contains_key(&id) {
                        out.push(NodeRef::Scheme(id));
                    } else {
                        *changed = true;
                    }
                }
                NodeRef::Folder(id) => {
                    *changed = true;
                    folders_to_remove.push(id);
                    self.collect_schemes_for_one_level_folder(id, out, folders_to_remove, changed);
                }
            }
        }
    }

    fn referenced_scheme_ids(&self) -> HashSet<SchemeId> {
        let mut ids = HashSet::new();
        for folder in self.folders.values() {
            for child in &folder.children {
                if let NodeRef::Scheme(id) = child {
                    ids.insert(*id);
                }
            }
        }
        ids
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scheme;

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
}
