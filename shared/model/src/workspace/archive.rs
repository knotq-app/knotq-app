use crate::{FolderId, Scheme, SchemeId};

use super::{DeletedFolderOrigin, DeletedSchemeOrigin, Folder, NodeRef, Workspace};

impl Workspace {
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

    pub fn is_folder_deleted(&self, id: FolderId) -> bool {
        self.recently_deleted_folders.contains(&id)
    }

    pub fn is_node_in_deleted_folder_subtree(&self, node: NodeRef) -> bool {
        self.deleted_folder_ancestor(node).is_some()
    }

    pub fn is_scheme_in_deleted_folder_subtree(&self, id: SchemeId) -> bool {
        self.is_node_in_deleted_folder_subtree(NodeRef::Scheme(id))
    }

    pub fn deleted_folder_ancestor(&self, node: NodeRef) -> Option<FolderId> {
        for top in &self.recently_deleted_folders {
            if self.subtree_contains_node(*top, node) {
                return Some(*top);
            }
        }
        None
    }

    /// The top-level archived folders, in archive order.
    pub fn iter_deleted_folders(&self) -> impl Iterator<Item = &Folder> {
        self.recently_deleted_folders
            .iter()
            .filter_map(|id| self.folders.get(id))
    }

    pub fn deleted_folder_origin(&self, id: FolderId) -> Option<DeletedFolderOrigin> {
        self.deleted_folder_origins.get(&id).copied()
    }

    /// Archive a folder as one unit: it (and its whole subtree) is kept in
    /// `folders`/`schemes` but detached from the sidebar, recorded as an archived
    /// top-level folder, and every scheme inside the subtree is marked deleted so the
    /// existing per-scheme archive checks still hold. The caller is responsible for
    /// having removed the folder from its parent's `children`.
    pub fn mark_folder_deleted_from(&mut self, id: FolderId, parent: FolderId, position: usize) {
        self.mark_folder_deleted_at(id, 0, DeletedFolderOrigin { parent, position });
    }

    pub fn mark_folder_deleted_at(
        &mut self,
        id: FolderId,
        position: usize,
        origin: DeletedFolderOrigin,
    ) {
        self.recently_deleted_folders
            .retain(|deleted| *deleted != id);
        let position = position.min(self.recently_deleted_folders.len());
        self.recently_deleted_folders.insert(position, id);
        self.deleted_folder_origins.insert(id, origin);
        for scheme in self.subtree_scheme_ids(id) {
            self.mark_scheme_deleted(scheme);
        }
    }

    pub fn unmark_folder_deleted_shallow(&mut self, id: FolderId) {
        self.recently_deleted_folders.retain(|folder| *folder != id);
        self.deleted_folder_origins.remove(&id);
    }

    pub fn remove_scheme_from_archive(&mut self, id: SchemeId) {
        self.recently_deleted.retain(|deleted| *deleted != id);
        self.deleted_scheme_origins.remove(&id);
    }

    pub fn remove_folder_from_archive(&mut self, id: FolderId) {
        if !self.recently_deleted_folders.contains(&id) {
            return;
        }
        self.unmark_folder_deleted_shallow(id);
    }

    /// Reverse [`mark_folder_deleted_from`]: clear the folder's archived state and
    /// un-delete every scheme in its subtree. Re-attaching the folder to its parent
    /// is the caller's responsibility.
    pub fn unmark_folder_deleted(&mut self, id: FolderId) {
        self.recently_deleted_folders.retain(|folder| *folder != id);
        self.deleted_folder_origins.remove(&id);
        for scheme in self.subtree_scheme_ids(id) {
            self.unmark_scheme_deleted(scheme);
        }
    }
}
