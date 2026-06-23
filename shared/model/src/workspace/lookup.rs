use std::collections::HashSet;

use chrono::NaiveDate;

use crate::{FolderId, Scheme, SchemeId};

use super::{Folder, NodeRef, Workspace};

impl Workspace {
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

    pub fn subtree_scheme_ids(&self, folder: FolderId) -> HashSet<SchemeId> {
        let mut schemes = HashSet::new();
        let mut stack = vec![folder];
        let mut seen_folders = HashSet::new();
        while let Some(current) = stack.pop() {
            if !seen_folders.insert(current) {
                continue;
            }
            if let Some(folder) = self.folders.get(&current) {
                for child in &folder.children {
                    match child {
                        NodeRef::Scheme(id) => {
                            schemes.insert(*id);
                        }
                        NodeRef::Folder(id) => stack.push(*id),
                    }
                }
            }
        }
        schemes
    }

    pub fn subtree_folder_ids(&self, folder: FolderId) -> HashSet<FolderId> {
        let mut folders = HashSet::new();
        let mut stack = vec![folder];
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

    pub(super) fn subtree_contains_node(&self, folder: FolderId, target: NodeRef) -> bool {
        if target == NodeRef::Folder(folder) {
            return true;
        }
        let mut stack = vec![folder];
        let mut seen_folders = HashSet::new();
        while let Some(current) = stack.pop() {
            if !seen_folders.insert(current) {
                continue;
            }
            let Some(folder) = self.folders.get(&current) else {
                continue;
            };
            for child in &folder.children {
                if *child == target {
                    return true;
                }
                if let NodeRef::Folder(id) = child {
                    stack.push(*id);
                }
            }
        }
        false
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
}
