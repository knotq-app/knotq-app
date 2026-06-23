use std::collections::HashSet;

use crate::{FolderId, SchemeId};

use super::{NodeRef, Workspace};

impl Workspace {
    fn archived_folder_subtree_ids(&self) -> HashSet<FolderId> {
        let mut folders = HashSet::new();
        let mut stack: Vec<FolderId> = self.recently_deleted_folders.clone();
        while let Some(current) = stack.pop() {
            if !folders.insert(current) {
                continue;
            }
            if let Some(folder) = self.folders.get(&current) {
                for child in &folder.children {
                    if let NodeRef::Folder(id) = child {
                        stack.push(*id);
                    }
                }
            }
        }
        folders
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

        // Archived folder subtrees are detached from root, so the root walk never
        // visits them; retain them explicitly (and drop archive entries whose folder
        // no longer exists) so an archived folder keeps its structure.
        let deleted_folders_before = self.recently_deleted_folders.len();
        self.recently_deleted_folders
            .retain(|id| self.folders.contains_key(id));
        if self.recently_deleted_folders.len() != deleted_folders_before {
            changed = true;
        }
        let archived_folders = self.archived_folder_subtree_ids();
        let archived_folder_ids: HashSet<FolderId> =
            self.recently_deleted_folders.iter().copied().collect();
        let folder_origins_before = self.deleted_folder_origins.len();
        self.deleted_folder_origins
            .retain(|id, _| archived_folder_ids.contains(id));
        if self.deleted_folder_origins.len() != folder_origins_before {
            changed = true;
        }
        // Schemes inside an archived subtree must stay marked deleted.
        let mut archived_subtree_schemes: HashSet<SchemeId> = HashSet::new();
        for id in &self.recently_deleted_folders {
            archived_subtree_schemes.extend(self.subtree_scheme_ids(*id));
        }
        for id in &archived_subtree_schemes {
            if self.schemes.contains_key(id) && !self.recently_deleted.contains(id) {
                self.recently_deleted.push(*id);
                changed = true;
            }
        }

        let before_folders = self.folders.len();
        self.folders.retain(|id, _| {
            *id == self.root || visited_folders.contains(id) || archived_folders.contains(id)
        });
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
            .chain(archived_subtree_schemes.iter().copied())
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
                        || self.recently_deleted_folders.contains(&id)
                    {
                        // Archived folders are detached from the sidebar tree.
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

    pub fn normalize_item_markers(&mut self) -> bool {
        let mut changed = false;
        for scheme in self.schemes.values_mut() {
            for item in &mut scheme.items {
                changed |= item.enforce_marker_constraints();
            }
        }
        changed
    }
}
