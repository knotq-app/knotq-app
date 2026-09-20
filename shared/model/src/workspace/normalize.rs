use std::collections::HashSet;

use crate::{FolderId, SchemeId, PERMANENT_DELETE_TOMBSTONE_POSITION};

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
            // An origin for an absent node is a permanent-delete tombstone. It
            // must survive normalization even though the corresponding archive
            // entry is gone: a stale replica can otherwise merge the old node
            // back into the workspace and make it live again.
            .retain(|id, origin| {
                archived_folder_ids.contains(id)
                    || !self.folders.contains_key(id)
                    || origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION
            });
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

        // A folder can become unreachable from the root without anyone deleting
        // it. `move_node` rejects a cycle it can SEE, but two devices that each
        // move one of two folders into the other are both issuing a legal
        // command against their own view, and only the merged result holds the
        // cycle. The walk above starts at the root and never reaches such a
        // pair, so the retain below would delete both folders AND every scheme
        // inside them, and the next index write would publish that as an
        // authoritative deletion for the whole account (production fuzz seed
        // 10042: three schemes and three folders off the server in one push).
        //
        // Break the cycle instead of dropping it, at its lowest id so every
        // replica picks the same member and they converge, then re-walk so the
        // rescued subtree's schemes count as referenced below. A chain that
        // simply ends — an ordinary orphan whose parent is gone — is left to the
        // retain, which is what normalization is meant to do with it.
        fn ancestry_cycles(workspace: &Workspace, start: FolderId) -> bool {
            let mut seen = HashSet::new();
            let mut current = Some(start);
            while let Some(id) = current {
                if !seen.insert(id) {
                    return true;
                }
                if id == workspace.root {
                    return false;
                }
                current = workspace.folders.get(&id).and_then(|folder| folder.parent);
            }
            false
        }

        for _ in 0..=self.folders.len() {
            let stranded: Vec<FolderId> = self
                .folders
                .keys()
                .copied()
                .filter(|id| {
                    *id != self.root
                        && !visited_folders.contains(id)
                        && !archived_folders.contains(id)
                })
                .collect();
            let Some(rescue) = stranded
                .into_iter()
                .filter(|id| ancestry_cycles(self, *id))
                .min()
            else {
                break;
            };
            let root = self.root;
            if let Some(parent) = self.folders.get(&rescue).and_then(|folder| folder.parent) {
                if let Some(parent) = self.folders.get_mut(&parent) {
                    parent
                        .children
                        .retain(|child| *child != NodeRef::Folder(rescue));
                }
            }
            if let Some(folder) = self.folders.get_mut(&rescue) {
                folder.parent = Some(root);
            }
            if let Some(root_folder) = self.folders.get_mut(&root) {
                if !root_folder.children.contains(&NodeRef::Folder(rescue)) {
                    root_folder.children.push(NodeRef::Folder(rescue));
                }
            }
            changed = true;
            visited_folders.clear();
            referenced_schemes.clear();
            self.normalize_folder_tree(
                root,
                None,
                &mut visited_folders,
                &mut referenced_schemes,
                &mut changed,
            );
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
            // See the folder-origin retention above. A missing scheme with an
            // origin is a durable permanent-delete tombstone, not an orphaned
            // bit of restore metadata.
            .retain(|id, origin| {
                deleted_ids.contains(id)
                    || !self.schemes.contains_key(id)
                    || origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION
            });
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

        // A scheme the folder tree does not mention is a structural anomaly,
        // not a deletion. Dropping it here is destructive twice over: the
        // scheme goes, and because the workspace index is then written from
        // this workspace, the drop is PUBLISHED to the account as an
        // authoritative deletion — every other device loses the scheme, and
        // its document lingers on the server as an orphan with no index entry
        // (production fuzz chaos seed 112: a scheme another device created
        // disappeared for the whole account after a third device normalized
        // its own view of the tree).
        //
        // A real deletion has evidence: the archive list, or a permanent-delete
        // tombstone in `deleted_scheme_origins`. Without either, re-home the
        // scheme under the root — the same choice the folder walk above makes
        // for a stranded folder — so normalization repairs structure and never
        // destroys content.
        let mut rescued: Vec<SchemeId> =
            self.schemes
                .keys()
                .copied()
                .filter(|id| !retained_schemes.contains(id))
                .filter(|id| {
                    !self.deleted_scheme_origins.get(id).is_some_and(|origin| {
                        origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION
                    })
                })
                .collect();
        // Deterministic order: two replicas normalizing the same anomaly must
        // rescue in the same order or the root's child list will not converge.
        rescued.sort();
        if !rescued.is_empty() {
            let root = self.root;
            if let Some(root_folder) = self.folders.get_mut(&root) {
                for id in &rescued {
                    if !root_folder.children.contains(&NodeRef::Scheme(*id)) {
                        root_folder.children.push(NodeRef::Scheme(*id));
                    }
                }
            }
            changed = true;
        }
        let kept_schemes: HashSet<SchemeId> = retained_schemes
            .iter()
            .copied()
            .chain(rescued.iter().copied())
            .collect();
        let before = self.schemes.len();
        self.schemes.retain(|id, _| kept_schemes.contains(id));
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

    /// Enforce the per-item marker invariants, reporting **which schemes**
    /// changed rather than merely whether any did.
    ///
    /// The caller needs the set, not a flag: this rewrites item content, and a
    /// repair that reaches only the plain workspace leaves it describing
    /// something its own CRDT documents do not hold. The sync path turns that
    /// difference into a local edit on the next pull and re-asserts the stale
    /// value over the merged one — a revert with no other device involved. See
    /// `queue_repair_crdt_updates` in the desktop sync service, which takes
    /// this set as its change set.
    pub fn normalize_item_markers(&mut self) -> HashSet<SchemeId> {
        let mut changed = HashSet::new();
        for (id, scheme) in self.schemes.iter_mut() {
            for item in &mut scheme.items {
                if item.enforce_marker_constraints() {
                    changed.insert(*id);
                }
            }
        }
        changed
    }
}
