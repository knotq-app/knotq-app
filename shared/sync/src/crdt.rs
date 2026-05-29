use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context};
use chrono::NaiveDate;
use knotq_model::{
    DeletedSchemeOrigin, DocumentId, Folder, FolderId, Item, Scheme, SchemeId, SchemeSource,
    SyncDocumentKind, SyncDocumentMeta, Workspace,
};
use serde::{Deserialize, Serialize};
use yrs::updates::{decoder::Decode, encoder::Encode};
use yrs::{Array, Doc, In, Map, MapPrelim, ReadTxn, StateVector, Transact, Update};

use crate::CrdtDocumentUpdate;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceCrdtChangeSet {
    pub workspace: bool,
    pub schemes: HashSet<SchemeId>,
}

impl WorkspaceCrdtChangeSet {
    pub fn workspace(mut self) -> Self {
        self.workspace = true;
        self
    }

    pub fn touch_scheme(mut self, scheme: SchemeId) -> Self {
        self.schemes.insert(scheme);
        self
    }

    pub fn merge(&mut self, other: Self) {
        self.workspace |= other.workspace;
        self.schemes.extend(other.schemes);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceCrdtSyncOutcome {
    pub updates: Vec<CrdtDocumentUpdate>,
    pub errors: Vec<String>,
}

impl WorkspaceCrdtSyncOutcome {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn push_error(&mut self, context: impl std::fmt::Display, error: anyhow::Error) {
        self.errors.push(format!("{context}: {error:#}"));
    }
}

pub struct WorkspaceCrdtDocuments {
    workspace: YrsJsonDocument,
    schemes: HashMap<SchemeId, YrsSchemeDocument>,
}

impl WorkspaceCrdtDocuments {
    pub fn empty(workspace: &Workspace) -> Self {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        Self {
            workspace: YrsJsonDocument::new(workspace.sync.id, SyncDocumentKind::PersonalWorkspace),
            schemes: HashMap::new(),
        }
    }

    pub fn try_new(workspace: &Workspace) -> anyhow::Result<Self> {
        let mut docs = Self::empty(workspace);
        docs.replace_all(workspace)?;
        Ok(docs)
    }

    pub fn replace_all(&mut self, workspace: &Workspace) -> anyhow::Result<()> {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        self.workspace
            .replace_snapshot(&workspace_document_snapshot(&workspace))?;

        self.schemes
            .retain(|id, _| workspace.schemes.contains_key(id));
        for (id, scheme) in &workspace.schemes {
            let meta = scheme_meta(&workspace, *id)?;
            self.schemes
                .entry(*id)
                .or_insert_with(|| YrsSchemeDocument::new(meta.id))
                .replace_scheme(scheme)
                .with_context(|| format!("replace scheme CRDT {id}"))?;
        }
        Ok(())
    }

    pub fn sync_changes(
        &mut self,
        workspace: &Workspace,
        changeset: &WorkspaceCrdtChangeSet,
    ) -> WorkspaceCrdtSyncOutcome {
        let mut workspace = workspace.clone();
        workspace.ensure_sync_metadata();
        let mut outcome = WorkspaceCrdtSyncOutcome::default();

        if changeset.workspace
            || documents_missing(self, &workspace)
            || documents_removed(self, &workspace)
        {
            match self
                .workspace
                .sync_snapshot(&workspace_document_snapshot(&workspace))
            {
                Ok(Some(update)) => outcome.updates.push(update),
                Ok(None) => {}
                Err(err) => outcome.push_error("workspace CRDT update", err),
            }
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
            let meta = match scheme_meta(&workspace, id) {
                Ok(meta) => meta,
                Err(err) => {
                    outcome.push_error(format!("scheme CRDT metadata {id}"), err);
                    continue;
                }
            };
            match self
                .schemes
                .entry(id)
                .or_insert_with(|| YrsSchemeDocument::new(meta.id))
                .sync_scheme(scheme)
            {
                Ok(Some(update)) => outcome.updates.push(update),
                Ok(None) => {}
                Err(err) => outcome.push_error(format!("scheme CRDT update {id}"), err),
            }
        }

        outcome
    }
}

fn documents_missing(docs: &WorkspaceCrdtDocuments, workspace: &Workspace) -> bool {
    workspace
        .schemes
        .keys()
        .any(|id| !docs.schemes.contains_key(id))
}

fn documents_removed(docs: &WorkspaceCrdtDocuments, workspace: &Workspace) -> bool {
    docs.schemes
        .keys()
        .any(|id| !workspace.schemes.contains_key(id))
}

pub struct YrsSchemeDocument {
    id: DocumentId,
    doc: Doc,
}

impl YrsSchemeDocument {
    pub fn new(id: DocumentId) -> Self {
        let doc = Doc::new();
        doc.get_or_insert_map("scheme_file");
        doc.get_or_insert_array("item_order");
        doc.get_or_insert_map("items_by_id");
        Self { id, doc }
    }

    pub fn from_scheme(id: DocumentId, scheme: &Scheme) -> anyhow::Result<Self> {
        let this = Self::new(id);
        this.replace_scheme(scheme)?;
        Ok(this)
    }

    pub fn sync_scheme(&self, scheme: &Scheme) -> anyhow::Result<Option<CrdtDocumentUpdate>> {
        let before = self.state_vector_v1();
        self.replace_scheme(scheme)?;
        let update_v1 = self.encode_update_v1(&before)?;
        if update_v1.is_empty() {
            return Ok(None);
        }
        Ok(Some(CrdtDocumentUpdate {
            document: self.id,
            kind: SyncDocumentKind::Scheme,
            update_v1,
        }))
    }

    pub fn replace_scheme(&self, scheme: &Scheme) -> anyhow::Result<()> {
        let metadata = self.doc.get_or_insert_map("scheme_file");
        let item_order = self.doc.get_or_insert_array("item_order");
        let items_by_id = self.doc.get_or_insert_map("items_by_id");
        let mut txn = self.doc.transact_mut();

        metadata.insert(&mut txn, "schema", "knotq.scheme_file.v1");
        metadata.insert(&mut txn, "id", scheme.id.to_string());

        let len = item_order.len(&txn);
        if len > 0 {
            item_order.remove_range(&mut txn, 0, len);
        }

        let retained = scheme
            .items
            .iter()
            .map(|item| item.id.to_string())
            .collect::<HashSet<_>>();
        let stale_keys = items_by_id
            .keys(&txn)
            .filter(|key| !retained.contains(*key))
            .map(str::to_string)
            .collect::<Vec<_>>();
        for key in stale_keys {
            items_by_id.remove(&mut txn, &key);
        }

        for item in &scheme.items {
            let item_id = item.id.to_string();
            item_order.push_back(&mut txn, item_id.clone());
            items_by_id.insert(&mut txn, item_id, item_prelim(item)?);
        }
        Ok(())
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
        let item_order = self.doc.get_or_insert_array("item_order");
        let items_by_id = self.doc.get_or_insert_map("items_by_id");
        let txn = self.doc.transact();
        let mut out = Vec::new();
        for index in 0..item_order.len(&txn) {
            let Some(item_id) = item_order.get_as::<_, Option<String>>(&txn, index)? else {
                continue;
            };
            if let Some(item) = items_by_id.get_as::<_, Option<YrsSchemeItem>>(&txn, &item_id)? {
                out.push(item.text);
            }
        }
        Ok(out)
    }
}

fn item_prelim(item: &Item) -> anyhow::Result<MapPrelim> {
    Ok(MapPrelim::from([
        ("schema", In::from("knotq.item.v2")),
        ("id", In::from(item.id.to_string())),
        ("text", In::from(item.text.clone())),
        ("marker", In::from(serde_json_string_value(&item.marker)?)),
        ("indent", In::from(i64::from(item.indent))),
        (
            "start",
            In::from(item.start.map(|dt| dt.to_rfc3339()).unwrap_or_default()),
        ),
        (
            "end",
            In::from(item.end.map(|dt| dt.to_rfc3339()).unwrap_or_default()),
        ),
        (
            "available",
            In::from(item.available.map(|dt| dt.to_rfc3339()).unwrap_or_default()),
        ),
        ("media_json", In::from(serde_json::to_string(&item.media)?)),
        (
            "repeats_json",
            In::from(serde_json::to_string(&item.repeats)?),
        ),
        ("state_json", In::from(serde_json::to_string(&item.state)?)),
        (
            "priority_json",
            In::from(serde_json::to_string(&item.priority)?),
        ),
        (
            "external_json",
            In::from(serde_json::to_string(&item.external)?),
        ),
        ("snapshot_json", In::from(serde_json::to_string(item)?)),
    ]))
}

fn serde_json_string_value(value: &impl Serialize) -> anyhow::Result<String> {
    let value = serde_json::to_value(value)?;
    Ok(value.as_str().unwrap_or_default().to_string())
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

    fn sync_snapshot(
        &self,
        snapshot: &impl Serialize,
    ) -> anyhow::Result<Option<CrdtDocumentUpdate>> {
        let before = self.doc.transact().state_vector().encode_v1();
        if !self.replace_snapshot(snapshot)? {
            return Ok(None);
        }
        let remote_state = StateVector::decode_v1(&before)?;
        let update_v1 = self.doc.transact().encode_diff_v1(&remote_state);
        if update_v1.is_empty() {
            return Ok(None);
        }
        Ok(Some(CrdtDocumentUpdate {
            document: self.id,
            kind: self.kind,
            update_v1,
        }))
    }

    fn replace_snapshot(&self, snapshot: &impl Serialize) -> anyhow::Result<bool> {
        let json = serde_json::to_string(snapshot)?;
        let document = self.doc.get_or_insert_map("document");
        let mut txn = self.doc.transact_mut();
        let existing = document.get_as::<_, Option<String>>(&txn, "snapshot")?;
        if existing.as_deref() == Some(json.as_str()) {
            return Ok(false);
        }
        document.insert(&mut txn, "snapshot", json);
        Ok(true)
    }
}

#[derive(Deserialize)]
struct YrsSchemeItem {
    text: String,
}

#[derive(Serialize)]
struct WorkspaceDocumentSnapshot {
    schema: &'static str,
    id: knotq_model::WorkspaceId,
    sync: SyncDocumentMeta,
    root: FolderId,
    folders: Vec<Folder>,
    schemes: Vec<SchemeWorkspaceEntry>,
    daily_queue: Vec<DailyQueueEntry>,
    recently_deleted: Vec<SchemeId>,
    deleted_scheme_origins: Vec<DeletedSchemeOriginEntry>,
    scheme_sync: Vec<SchemeSyncEntry>,
    folder_sync: Vec<FolderSyncEntry>,
}

#[derive(Serialize)]
struct SchemeWorkspaceEntry {
    id: SchemeId,
    name: String,
    color_index: u8,
    gsync: bool,
    source: SchemeSource,
}

#[derive(Serialize)]
struct DailyQueueEntry {
    date: NaiveDate,
    scheme: SchemeId,
}

#[derive(Serialize)]
struct DeletedSchemeOriginEntry {
    scheme: SchemeId,
    origin: DeletedSchemeOrigin,
}

#[derive(Serialize)]
struct SchemeSyncEntry {
    scheme: SchemeId,
    sync: SyncDocumentMeta,
}

#[derive(Serialize)]
struct FolderSyncEntry {
    folder: FolderId,
    sync: SyncDocumentMeta,
}

fn workspace_document_snapshot(workspace: &Workspace) -> WorkspaceDocumentSnapshot {
    let mut folders = workspace.folders.values().cloned().collect::<Vec<_>>();
    folders.sort_by_key(|folder| folder.id.to_string());

    let mut schemes = workspace
        .schemes
        .values()
        .map(|scheme| SchemeWorkspaceEntry {
            id: scheme.id,
            name: scheme.name.clone(),
            color_index: scheme.color_index,
            gsync: scheme.gsync,
            source: scheme.source.clone(),
        })
        .collect::<Vec<_>>();
    schemes.sort_by_key(|scheme| scheme.id.to_string());

    let daily_queue = workspace
        .daily_queue
        .iter()
        .map(|(date, scheme)| DailyQueueEntry {
            date: *date,
            scheme: *scheme,
        })
        .collect::<Vec<_>>();

    let mut deleted_scheme_origins = workspace
        .deleted_scheme_origins
        .iter()
        .map(|(scheme, origin)| DeletedSchemeOriginEntry {
            scheme: *scheme,
            origin: *origin,
        })
        .collect::<Vec<_>>();
    deleted_scheme_origins.sort_by_key(|entry| entry.scheme.to_string());

    let mut scheme_sync = workspace
        .scheme_sync
        .iter()
        .map(|(scheme, sync)| SchemeSyncEntry {
            scheme: *scheme,
            sync: sync.clone(),
        })
        .collect::<Vec<_>>();
    scheme_sync.sort_by_key(|entry| entry.scheme.to_string());

    let mut folder_sync = workspace
        .folder_sync
        .iter()
        .map(|(folder, sync)| FolderSyncEntry {
            folder: *folder,
            sync: sync.clone(),
        })
        .collect::<Vec<_>>();
    folder_sync.sort_by_key(|entry| entry.folder.to_string());

    WorkspaceDocumentSnapshot {
        schema: "knotq.workspace.v1",
        id: workspace.id,
        sync: workspace.sync.clone(),
        root: workspace.root,
        folders,
        schemes,
        daily_queue,
        recently_deleted: workspace.recently_deleted.clone(),
        deleted_scheme_origins,
        scheme_sync,
        folder_sync,
    }
}

fn scheme_meta(workspace: &Workspace, id: SchemeId) -> anyhow::Result<&SyncDocumentMeta> {
    workspace
        .scheme_sync
        .get(&id)
        .ok_or_else(|| anyhow!("workspace missing scheme sync metadata for {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::{Item, NodeRef};

    #[test]
    fn scheme_document_update_can_be_applied_to_empty_replica() {
        let document = DocumentId::new();
        let mut scheme = Scheme::new("Plan", 0);
        scheme.items.push(Item::new("First"));
        scheme.items.push(Item::new("Second"));

        let left = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();
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

        let mut docs = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        workspace.schemes.get_mut(&scheme_id).unwrap().items[0].text = "Changed".to_string();
        let updates = docs
            .sync_changes(
                &workspace,
                &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
            )
            .updates;

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

        let mut docs = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        workspace.schemes.remove(&scheme_id);
        workspace.recently_deleted.retain(|id| *id != scheme_id);
        workspace.ensure_sync_metadata();

        let updates = docs
            .sync_changes(
                &workspace,
                &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
            )
            .updates;

        assert!(updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::PersonalWorkspace));
    }

    #[test]
    fn folder_changes_emit_workspace_document_not_folder_documents() {
        let mut workspace = Workspace::new();
        let folder = Folder {
            id: FolderId::new(),
            name: "Projects".to_string(),
            parent: Some(workspace.root),
            children: Vec::new(),
            expanded: true,
        };
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(folder.id));
        workspace.folders.insert(folder.id, folder);
        workspace.ensure_sync_metadata();

        let mut docs = WorkspaceCrdtDocuments::empty(&workspace);
        let updates = docs
            .sync_changes(&workspace, &WorkspaceCrdtChangeSet::default().workspace())
            .updates;

        assert!(updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::PersonalWorkspace));
        assert!(!updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::Folder));
    }
}
