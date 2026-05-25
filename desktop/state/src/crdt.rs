use std::collections::{HashMap, HashSet};

use knotq_commands::ChangeSet;
use knotq_model::{
    DocumentId, Folder, FolderId, Scheme, SchemeId, SyncDocumentKind, SyncDocumentMeta, Workspace,
};
use serde::{Deserialize, Serialize};
use yrs::updates::{decoder::Decode, encoder::Encode};
use yrs::{Array, Doc, In, Map, MapPrelim, ReadTxn, StateVector, Transact, Update};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrdtDocumentUpdate {
    pub document: DocumentId,
    pub kind: SyncDocumentKind,
    pub update_v1: Vec<u8>,
}

pub struct WorkspaceCrdtDocuments {
    workspace: YrsJsonDocument,
    schemes: HashMap<SchemeId, YrsSchemeDocument>,
    folders: HashMap<FolderId, YrsJsonDocument>,
}

impl WorkspaceCrdtDocuments {
    pub fn new(workspace: &Workspace) -> Self {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        let mut docs = Self {
            workspace: YrsJsonDocument::new(workspace.sync.id, SyncDocumentKind::PersonalWorkspace),
            schemes: HashMap::new(),
            folders: HashMap::new(),
        };
        docs.replace_all(&workspace);
        docs
    }

    pub fn replace_all(&mut self, workspace: &Workspace) {
        self.workspace
            .replace_snapshot(&workspace_layout_snapshot(workspace));

        self.schemes
            .retain(|id, _| workspace.schemes.contains_key(id));
        for (id, scheme) in &workspace.schemes {
            let meta = scheme_meta(workspace, *id);
            self.schemes
                .entry(*id)
                .or_insert_with(|| YrsSchemeDocument::new(meta.id))
                .replace_scheme(scheme);
        }

        self.folders
            .retain(|id, _| workspace.folders.contains_key(id));
        for (id, folder) in &workspace.folders {
            let meta = folder_meta(workspace, *id);
            self.folders
                .entry(*id)
                .or_insert_with(|| YrsJsonDocument::new(meta.id, SyncDocumentKind::Folder))
                .replace_snapshot(&FolderSnapshot { folder });
        }
    }

    pub fn sync_changes(
        &mut self,
        workspace: &Workspace,
        changeset: &ChangeSet,
    ) -> Vec<CrdtDocumentUpdate> {
        let mut updates = Vec::new();
        if !changeset.folders.is_empty()
            || documents_missing(self, workspace)
            || documents_removed(self, workspace)
        {
            updates.push(
                self.workspace
                    .sync_snapshot(&workspace_layout_snapshot(workspace)),
            );
        }

        let mut scheme_ids: HashSet<SchemeId> = changeset.schemes.iter().copied().collect();
        scheme_ids.extend(
            workspace
                .schemes
                .keys()
                .copied()
                .filter(|id| !self.schemes.contains_key(id)),
        );
        self.schemes
            .retain(|id, _| workspace.schemes.contains_key(id));
        for id in scheme_ids {
            let Some(scheme) = workspace.schemes.get(&id) else {
                continue;
            };
            let meta = scheme_meta(workspace, id);
            updates.push(
                self.schemes
                    .entry(id)
                    .or_insert_with(|| YrsSchemeDocument::new(meta.id))
                    .sync_scheme(scheme),
            );
        }

        let mut folder_ids: HashSet<FolderId> = changeset.folders.iter().copied().collect();
        folder_ids.extend(
            workspace
                .folders
                .keys()
                .copied()
                .filter(|id| !self.folders.contains_key(id)),
        );
        self.folders
            .retain(|id, _| workspace.folders.contains_key(id));
        for id in folder_ids {
            let Some(folder) = workspace.folders.get(&id) else {
                continue;
            };
            let meta = folder_meta(workspace, id);
            updates.push(
                self.folders
                    .entry(id)
                    .or_insert_with(|| YrsJsonDocument::new(meta.id, SyncDocumentKind::Folder))
                    .sync_snapshot(&FolderSnapshot { folder }),
            );
        }

        updates.retain(|update| !update.update_v1.is_empty());
        updates
    }
}

fn documents_missing(docs: &WorkspaceCrdtDocuments, workspace: &Workspace) -> bool {
    workspace
        .schemes
        .keys()
        .any(|id| !docs.schemes.contains_key(id))
        || workspace
            .folders
            .keys()
            .any(|id| !docs.folders.contains_key(id))
}

fn documents_removed(docs: &WorkspaceCrdtDocuments, workspace: &Workspace) -> bool {
    docs.schemes
        .keys()
        .any(|id| !workspace.schemes.contains_key(id))
        || docs
            .folders
            .keys()
            .any(|id| !workspace.folders.contains_key(id))
}

pub struct YrsSchemeDocument {
    id: DocumentId,
    doc: Doc,
}

impl YrsSchemeDocument {
    pub fn new(id: DocumentId) -> Self {
        let doc = Doc::new();
        doc.get_or_insert_array("items");
        Self { id, doc }
    }

    pub fn from_scheme(id: DocumentId, scheme: &Scheme) -> Self {
        let this = Self::new(id);
        this.replace_scheme(scheme);
        this
    }

    pub fn sync_scheme(&self, scheme: &Scheme) -> CrdtDocumentUpdate {
        let before = self.state_vector_v1();
        self.replace_scheme(scheme);
        CrdtDocumentUpdate {
            document: self.id,
            kind: SyncDocumentKind::Scheme,
            update_v1: self
                .encode_update_v1(&before)
                .expect("encode Yrs scheme update"),
        }
    }

    pub fn replace_scheme(&self, scheme: &Scheme) {
        let items = self.doc.get_or_insert_array("items");
        let mut txn = self.doc.transact_mut();
        let len = items.len(&txn);
        if len > 0 {
            items.remove_range(&mut txn, 0, len);
        }
        for item in &scheme.items {
            let item_json = serde_json::to_string(item).expect("serialize scheme item");
            items.push_back(
                &mut txn,
                MapPrelim::from([
                    ("id", In::from(item.id.to_string())),
                    ("text", In::from(item.text.clone())),
                    ("item", In::from(item_json)),
                ]),
            );
        }
    }

    pub fn state_vector_v1(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    pub fn encode_update_v1(&self, remote_state_vector: &[u8]) -> anyhow::Result<Vec<u8>> {
        let remote_state = if remote_state_vector.is_empty() {
            StateVector::default()
        } else {
            StateVector::decode_v1(remote_state_vector)?
        };
        Ok(self.doc.transact().encode_diff_v1(&remote_state))
    }

    pub fn apply_update_v1(&self, update: &[u8]) -> anyhow::Result<()> {
        self.doc
            .transact_mut()
            .apply_update(Update::decode_v1(update)?)?;
        Ok(())
    }

    pub fn item_texts(&self) -> anyhow::Result<Vec<String>> {
        let items = self.doc.get_or_insert_array("items");
        let txn = self.doc.transact();
        let mut out = Vec::new();
        for index in 0..items.len(&txn) {
            if let Some(item) = items.get_as::<_, Option<YrsSchemeItem>>(&txn, index)? {
                out.push(item.text);
            }
        }
        Ok(out)
    }
}

struct YrsJsonDocument {
    id: DocumentId,
    kind: SyncDocumentKind,
    doc: Doc,
}

impl YrsJsonDocument {
    fn new(id: DocumentId, kind: SyncDocumentKind) -> Self {
        let doc = Doc::new();
        doc.get_or_insert_map("document");
        Self { id, kind, doc }
    }

    fn sync_snapshot(&self, snapshot: &impl Serialize) -> CrdtDocumentUpdate {
        let before = self.doc.transact().state_vector().encode_v1();
        self.replace_snapshot(snapshot);
        let remote_state = StateVector::decode_v1(&before).expect("decode Yrs state vector");
        CrdtDocumentUpdate {
            document: self.id,
            kind: self.kind,
            update_v1: self.doc.transact().encode_diff_v1(&remote_state),
        }
    }

    fn replace_snapshot(&self, snapshot: &impl Serialize) {
        let json = serde_json::to_string(snapshot).expect("serialize CRDT snapshot");
        let document = self.doc.get_or_insert_map("document");
        document.insert(&mut self.doc.transact_mut(), "snapshot", json);
    }
}

#[derive(Deserialize)]
struct YrsSchemeItem {
    text: String,
}

#[derive(Serialize)]
struct WorkspaceLayoutSnapshot<'a> {
    root: FolderId,
    folders: &'a HashMap<FolderId, Folder>,
    daily_queue: &'a std::collections::BTreeMap<chrono::NaiveDate, SchemeId>,
    recently_deleted: &'a Vec<SchemeId>,
    deleted_scheme_origins: &'a HashMap<SchemeId, knotq_model::DeletedSchemeOrigin>,
    scheme_sync: &'a HashMap<SchemeId, SyncDocumentMeta>,
    folder_sync: &'a HashMap<FolderId, SyncDocumentMeta>,
}

#[derive(Serialize)]
struct FolderSnapshot<'a> {
    folder: &'a Folder,
}

fn workspace_layout_snapshot(workspace: &Workspace) -> WorkspaceLayoutSnapshot<'_> {
    WorkspaceLayoutSnapshot {
        root: workspace.root,
        folders: &workspace.folders,
        daily_queue: &workspace.daily_queue,
        recently_deleted: &workspace.recently_deleted,
        deleted_scheme_origins: &workspace.deleted_scheme_origins,
        scheme_sync: &workspace.scheme_sync,
        folder_sync: &workspace.folder_sync,
    }
}

fn scheme_meta(workspace: &Workspace, id: SchemeId) -> &SyncDocumentMeta {
    workspace
        .scheme_sync
        .get(&id)
        .expect("workspace missing scheme sync metadata")
}

fn folder_meta(workspace: &Workspace, id: FolderId) -> &SyncDocumentMeta {
    workspace
        .folder_sync
        .get(&id)
        .expect("workspace missing folder sync metadata")
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::{Item, Scheme};

    #[test]
    fn scheme_document_update_can_be_applied_to_empty_replica() {
        let document = DocumentId::new();
        let mut scheme = Scheme::new("Plan", 0);
        scheme.items.push(Item::new("First"));
        scheme.items.push(Item::new("Second"));

        let left = YrsSchemeDocument::from_scheme(document, &scheme);
        let right = YrsSchemeDocument::new(document);
        let update = left.encode_update_v1(&right.state_vector_v1()).unwrap();

        right.apply_update_v1(&update).unwrap();

        assert_eq!(right.item_texts().unwrap(), vec!["First", "Second"]);
    }

    #[test]
    fn workspace_crdt_documents_emit_scheme_updates_for_touched_schemes() {
        let mut workspace = Workspace::new();
        let mut scheme = Scheme::new("Plan", 0);
        scheme.items.push(Item::new("First"));
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();

        let mut docs = WorkspaceCrdtDocuments::new(&workspace);
        workspace.schemes.get_mut(&scheme_id).unwrap().items[0].text = "Changed".to_string();
        let updates =
            docs.sync_changes(&workspace, &ChangeSet::default().touched_scheme(scheme_id));

        assert!(updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::Scheme));
    }

    #[test]
    fn workspace_crdt_documents_emit_workspace_updates_for_removed_schemes() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Plan", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.mark_scheme_deleted(scheme_id);
        workspace.ensure_sync_metadata();

        let mut docs = WorkspaceCrdtDocuments::new(&workspace);
        workspace.schemes.remove(&scheme_id);
        workspace.recently_deleted.retain(|id| *id != scheme_id);
        workspace.ensure_sync_metadata();

        let updates =
            docs.sync_changes(&workspace, &ChangeSet::default().touched_scheme(scheme_id));

        assert!(updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::PersonalWorkspace));
    }
}
