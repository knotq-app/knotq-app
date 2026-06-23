use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use crate::{
    daily_queue_scheme_id, daily_queue_sync_metadata, default_folder_sync, default_scheme_sync,
    CrdtBackend, DocumentId, FolderId, SchemeId, SyncDocumentKind, WorkspaceId,
};

use super::{
    daily_queue_date_from_scheme_name, dedupe_node_refs, merge_daily_queue_scheme,
    personal_workspace_root_folder_id, Folder, NodeRef, Workspace,
};

impl Workspace {
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
        for origin in self.deleted_folder_origins.values_mut() {
            if origin.parent == old_root || merge_roots.contains(&origin.parent) {
                origin.parent = expected_root;
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
