use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context};
use chrono::{DateTime, NaiveDate};
use knotq_model::{
    DeletedSchemeOrigin, DocumentId, Folder, FolderId, Item, ItemId, Scheme, SchemeId,
    SchemeSource, SyncDocumentKind, SyncDocumentMeta, Workspace,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use yrs::updates::{decoder::Decode, encoder::Encode};
use yrs::{Doc, In, Map, MapPrelim, ReadTxn, StateVector, Transact, Update};

use crate::{CrdtDocumentUpdate, StoredCrdtUpdate};

const SCHEME_SCHEMA_V2: &str = "knotq.scheme_file.v2";

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

#[derive(Clone, Debug)]
pub struct WorkspaceCrdtApplyOutcome {
    pub workspace: Workspace,
    pub applied: usize,
    pub errors: Vec<String>,
}

impl WorkspaceCrdtApplyOutcome {
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

pub fn validate_crdt_update_sequence<'a>(
    kind: SyncDocumentKind,
    updates_v1: impl IntoIterator<Item = &'a [u8]>,
) -> anyhow::Result<()> {
    let doc = Doc::new();
    for update in updates_v1 {
        doc.transact_mut()
            .apply_update(Update::decode_v1(update).context("decode update_v1")?)
            .context("apply update_v1")?;
    }

    match kind {
        SyncDocumentKind::PersonalWorkspace => validate_workspace_document(&doc),
        SyncDocumentKind::Scheme => validate_scheme_document(&doc),
        SyncDocumentKind::Folder => Err(anyhow!("folder CRDT documents are not supported")),
    }
}

impl WorkspaceCrdtDocuments {
    pub fn snapshot_updates(workspace: &Workspace) -> WorkspaceCrdtSyncOutcome {
        let mut docs = Self::empty(workspace);
        docs.sync_changes(workspace, &WorkspaceCrdtChangeSet::default().workspace())
    }

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

    pub fn apply_remote_updates(
        &mut self,
        current: &Workspace,
        updates: &[StoredCrdtUpdate],
    ) -> WorkspaceCrdtApplyOutcome {
        let mut outcome = WorkspaceCrdtApplyOutcome {
            workspace: current.clone(),
            applied: 0,
            errors: Vec::new(),
        };

        let mut workspace_applied = false;
        for update in updates
            .iter()
            .filter(|update| update.kind == SyncDocumentKind::PersonalWorkspace)
        {
            if update.document != self.workspace.id {
                outcome.push_error(
                    format!("workspace update {}", update.sequence),
                    anyhow!(
                        "document id mismatch: expected {}, got {}",
                        self.workspace.id,
                        update.document
                    ),
                );
                continue;
            }
            match self.workspace.apply_update_v1(&update.update_v1) {
                Ok(()) => {
                    outcome.applied += 1;
                    workspace_applied = true;
                }
                Err(err) => {
                    outcome.push_error(format!("workspace update {}", update.sequence), err)
                }
            }
        }

        // Defense in depth: the client does not blindly trust remote bytes. After
        // applying remote updates, re-run the same schema validation the server
        // performs before materializing/persisting anything.
        if workspace_applied {
            if let Err(err) = self.workspace.validate() {
                outcome.push_error("workspace validation", err);
                return outcome;
            }
        }

        match self.materialize_workspace(current) {
            Ok(workspace) => outcome.workspace = workspace,
            Err(err) => {
                outcome.push_error("workspace materialization", err);
                return outcome;
            }
        }

        self.schemes
            .retain(|id, _| outcome.workspace.schemes.contains_key(id));
        let scheme_by_document = scheme_documents_by_id(&outcome.workspace);
        let mut touched_schemes: HashSet<SchemeId> = HashSet::new();
        for update in updates
            .iter()
            .filter(|update| update.kind == SyncDocumentKind::Scheme)
        {
            let Some(scheme_id) = scheme_by_document.get(&update.document).copied() else {
                outcome.push_error(
                    format!("scheme update {}", update.sequence),
                    anyhow!("unknown scheme document {}", update.document),
                );
                continue;
            };
            match self
                .schemes
                .entry(scheme_id)
                .or_insert_with(|| YrsSchemeDocument::new(update.document))
                .apply_update_v1(&update.update_v1)
            {
                Ok(()) => {
                    outcome.applied += 1;
                    touched_schemes.insert(scheme_id);
                }
                Err(err) => outcome.push_error(format!("scheme update {}", update.sequence), err),
            }
        }

        for scheme_id in &touched_schemes {
            if let Some(doc) = self.schemes.get(scheme_id) {
                if let Err(err) = doc.validate() {
                    outcome.push_error(format!("scheme validation {scheme_id}"), err);
                }
            }
        }

        if !touched_schemes.is_empty() {
            match self.materialize_workspace(current) {
                Ok(workspace) => outcome.workspace = workspace,
                Err(err) => outcome.push_error("scheme materialization", err),
            }
        }

        for update in updates
            .iter()
            .filter(|update| update.kind == SyncDocumentKind::Folder)
        {
            outcome.push_error(
                format!("folder update {}", update.sequence),
                anyhow!("folder CRDT documents are not supported"),
            );
        }

        outcome
    }

    fn materialize_workspace(&self, current: &Workspace) -> anyhow::Result<Workspace> {
        let snapshot: WorkspaceDocumentSnapshot = self.workspace.snapshot()?;
        let scheme_sync = snapshot
            .scheme_sync
            .into_iter()
            .map(|entry| (entry.scheme, entry.sync))
            .collect::<HashMap<_, _>>();
        let folder_sync = snapshot
            .folder_sync
            .into_iter()
            .map(|entry| (entry.folder, entry.sync))
            .collect::<HashMap<_, _>>();
        let mut workspace = Workspace {
            id: snapshot.id,
            sync: snapshot.sync,
            root: snapshot.root,
            folders: snapshot
                .folders
                .into_iter()
                .map(|folder| (folder.id, folder))
                .collect(),
            schemes: HashMap::new(),
            scheme_sync,
            folder_sync,
            daily_queue: snapshot
                .daily_queue
                .into_iter()
                .map(|entry| (entry.date, entry.scheme))
                .collect(),
            recently_deleted: snapshot.recently_deleted,
            deleted_scheme_origins: snapshot
                .deleted_scheme_origins
                .into_iter()
                .map(|entry| (entry.scheme, entry.origin))
                .collect(),
        };

        for entry in snapshot.schemes {
            let items = self
                .schemes
                .get(&entry.id)
                .and_then(|doc| doc.scheme_items().ok())
                .or_else(|| {
                    current
                        .schemes
                        .get(&entry.id)
                        .map(|scheme| scheme.items.clone())
                })
                .unwrap_or_default();
            workspace.schemes.insert(
                entry.id,
                Scheme {
                    id: entry.id,
                    name: entry.name,
                    color_index: entry.color_index,
                    gsync: entry.gsync,
                    source: entry.source,
                    items,
                },
            );
        }

        workspace.ensure_sync_metadata();
        Ok(workspace)
    }
}

fn validate_workspace_document(doc: &Doc) -> anyhow::Result<()> {
    let document = doc.get_or_insert_map("document");
    let txn = doc.transact();
    let snapshot = document
        .get_as::<_, Option<String>>(&txn, "snapshot")
        .context("read workspace snapshot")?
        .ok_or_else(|| anyhow!("workspace snapshot missing"))?;
    let snapshot: serde_json::Value =
        serde_json::from_str(&snapshot).context("workspace snapshot is not JSON")?;
    let object = snapshot
        .as_object()
        .ok_or_else(|| anyhow!("workspace snapshot is not an object"))?;
    require_json_string(object, "schema", "knotq.workspace.v1")?;
    parse_json_uuid(object, "id")?;
    parse_json_uuid(object, "root")?;
    require_json_object(object, "sync")?;
    require_json_array(object, "folders")?;
    require_json_array(object, "schemes")?;
    require_json_array(object, "daily_queue")?;
    require_json_array(object, "recently_deleted")?;
    require_json_array(object, "deleted_scheme_origins")?;
    require_json_array(object, "scheme_sync")?;
    require_json_array(object, "folder_sync")?;
    Ok(())
}

fn validate_scheme_document(doc: &Doc) -> anyhow::Result<()> {
    let metadata = doc.get_or_insert_map("scheme_file");
    let items_by_id = doc.get_or_insert_map("items_by_id");
    let txn = doc.transact();

    let schema = metadata
        .get_as::<_, Option<String>>(&txn, "schema")
        .context("read scheme schema")?
        .ok_or_else(|| anyhow!("scheme schema missing"))?;
    if schema != SCHEME_SCHEMA_V2 {
        return Err(anyhow!("scheme schema invalid"));
    }
    let scheme_id = metadata
        .get_as::<_, Option<String>>(&txn, "id")
        .context("read scheme id")?
        .ok_or_else(|| anyhow!("scheme id missing"))?;
    scheme_id.parse::<SchemeId>().context("scheme id invalid")?;

    // Items are keyed by id in the map, so id uniqueness is structural — there is
    // no separate order array to keep consistent or to duplicate under merge.
    let item_keys = items_by_id
        .keys(&txn)
        .map(str::to_string)
        .collect::<Vec<_>>();
    for item_id in item_keys {
        let parsed_item_id = item_id
            .parse::<ItemId>()
            .with_context(|| format!("item id invalid: {item_id}"))?;
        let item = items_by_id
            .get_as::<_, Option<YrsSchemeItemSnapshot>>(&txn, &item_id)
            .with_context(|| format!("read item {item_id}"))?
            .ok_or_else(|| anyhow!("item entry missing: {item_id}"))?;
        validate_scheme_item(&item_id, parsed_item_id, item)?;
    }

    Ok(())
}

fn validate_scheme_item(
    item_id: &str,
    parsed_item_id: ItemId,
    item: YrsSchemeItemSnapshot,
) -> anyhow::Result<()> {
    if item.schema != "knotq.item.v2" {
        return Err(anyhow!("item schema invalid: {item_id}"));
    }
    if item.id != item_id {
        return Err(anyhow!("item id mismatch: {item_id}"));
    }
    item.id.parse::<ItemId>().context("item id invalid")?;
    if item.position.is_empty() {
        return Err(anyhow!("item position missing: {item_id}"));
    }
    if !matches!(
        item.marker.as_str(),
        "blank" | "bullet" | "numbered" | "checkbox"
    ) {
        return Err(anyhow!("item marker invalid: {item_id}"));
    }
    if !(0..=i64::from(u8::MAX)).contains(&item.indent) {
        return Err(anyhow!("item indent invalid: {item_id}"));
    }
    parse_optional_rfc3339(&item.start)
        .with_context(|| format!("item start invalid: {item_id}"))?;
    parse_optional_rfc3339(&item.end).with_context(|| format!("item end invalid: {item_id}"))?;
    parse_optional_rfc3339(&item.available)
        .with_context(|| format!("item available invalid: {item_id}"))?;
    parse_json_value(&item.media_json).with_context(|| format!("item media invalid: {item_id}"))?;
    parse_json_value(&item.repeats_json)
        .with_context(|| format!("item repeats invalid: {item_id}"))?;
    parse_json_value(&item.state_json).with_context(|| format!("item state invalid: {item_id}"))?;
    parse_json_value(&item.priority_json)
        .with_context(|| format!("item priority invalid: {item_id}"))?;
    parse_json_value(&item.external_json)
        .with_context(|| format!("item external invalid: {item_id}"))?;

    let snapshot: Item = serde_json::from_str(&item.snapshot_json)
        .with_context(|| format!("item snapshot invalid: {item_id}"))?;
    if snapshot.id != parsed_item_id {
        return Err(anyhow!("item snapshot id mismatch: {item_id}"));
    }
    if snapshot.text != item.text {
        return Err(anyhow!("item snapshot text mismatch: {item_id}"));
    }

    Ok(())
}

fn parse_optional_rfc3339(value: &str) -> anyhow::Result<()> {
    if !value.is_empty() {
        DateTime::parse_from_rfc3339(value)?;
    }
    Ok(())
}

fn parse_json_value(value: &str) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::from_str(value)?)
}

fn require_json_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    expected: &str,
) -> anyhow::Result<()> {
    match object.get(key).and_then(serde_json::Value::as_str) {
        Some(actual) if actual == expected => Ok(()),
        Some(_) => Err(anyhow!("{key} invalid")),
        None => Err(anyhow!("{key} missing")),
    }
}

fn require_json_object(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> anyhow::Result<()> {
    object
        .get(key)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("{key} must be an object"))?;
    Ok(())
}

fn require_json_array(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> anyhow::Result<()> {
    object
        .get(key)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("{key} must be an array"))?;
    Ok(())
}

fn parse_json_uuid(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> anyhow::Result<()> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("{key} missing"))?
        .parse::<uuid::Uuid>()
        .with_context(|| format!("{key} invalid"))?;
    Ok(())
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
        let items_by_id = self.doc.get_or_insert_map("items_by_id");
        let mut txn = self.doc.transact_mut();

        if metadata
            .get_as::<_, Option<String>>(&txn, "schema")
            .ok()
            .flatten()
            .as_deref()
            != Some(SCHEME_SCHEMA_V2)
        {
            metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V2);
        }
        let scheme_id = scheme.id.to_string();
        if metadata
            .get_as::<_, Option<String>>(&txn, "id")
            .ok()
            .flatten()
            .as_deref()
            != Some(scheme_id.as_str())
        {
            metadata.insert(&mut txn, "id", scheme_id);
        }

        // Snapshot what is currently stored so we can reuse positions and skip
        // unchanged entries.
        let stored_keys = items_by_id
            .keys(&txn)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut stored: HashMap<String, YrsSchemeItemSnapshot> = HashMap::new();
        for key in stored_keys {
            if let Some(entry) = items_by_id
                .get_as::<_, Option<YrsSchemeItemSnapshot>>(&txn, &key)
                .ok()
                .flatten()
            {
                stored.insert(key, entry);
            }
        }

        // Assign each item a fractional `position`. Ordering lives on the item,
        // not in a shared array, so concurrent inserts/reorders merge without the
        // duplicate-id wedge. Keep an existing position whenever it still sorts
        // after the previous item; otherwise mint a fresh key between neighbors.
        let desired = scheme
            .items
            .iter()
            .map(|i| i.id.to_string())
            .collect::<Vec<_>>();
        let mut positions: Vec<String> = Vec::with_capacity(desired.len());
        for (idx, id) in desired.iter().enumerate() {
            let prev = positions.last().cloned();
            let existing = stored.get(id).map(|entry| entry.position.clone());
            let keep = match (&existing, &prev) {
                (Some(existing), Some(prev)) => existing.as_str() > prev.as_str(),
                (Some(_), None) => true,
                (None, _) => false,
            };
            let position = if keep {
                existing.unwrap()
            } else {
                let upper = desired[idx + 1..].iter().find_map(|next_id| {
                    stored
                        .get(next_id)
                        .map(|entry| entry.position.clone())
                        .filter(|candidate| {
                            prev.as_deref().is_none_or(|prev| candidate.as_str() > prev)
                        })
                });
                crate::fractional::between(prev.as_deref(), upper.as_deref())
            };
            positions.push(position);
        }

        let retained = desired.iter().cloned().collect::<HashSet<_>>();
        let stale_keys = items_by_id
            .keys(&txn)
            .filter(|key| !retained.contains(*key))
            .map(str::to_string)
            .collect::<Vec<_>>();
        for key in stale_keys {
            items_by_id.remove(&mut txn, &key);
        }

        // Re-insert an entry only when its content or its position changed, so a
        // single edit produces a single map-entry delta.
        for (item, position) in scheme.items.iter().zip(&positions) {
            let item_id = item.id.to_string();
            let next_snapshot = serde_json::to_string(item)?;
            let unchanged = stored.get(&item_id).is_some_and(|existing| {
                existing.snapshot_json == next_snapshot && existing.position == *position
            });
            if !unchanged {
                items_by_id.insert(&mut txn, item_id, item_prelim(item, position)?);
            }
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

    fn validate(&self) -> anyhow::Result<()> {
        validate_scheme_document(&self.doc)
    }

    /// All stored item entries sorted by `(position, item_id)`. The id breaks
    /// ties so replicas that independently generated the same fractional key
    /// still converge to one deterministic order.
    fn sorted_entries(&self) -> anyhow::Result<Vec<(String, YrsSchemeItemSnapshot)>> {
        let items_by_id = self.doc.get_or_insert_map("items_by_id");
        let txn = self.doc.transact();
        let keys = items_by_id
            .keys(&txn)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut entries = Vec::with_capacity(keys.len());
        for key in keys {
            let entry = items_by_id
                .get_as::<_, Option<YrsSchemeItemSnapshot>>(&txn, &key)?
                .ok_or_else(|| anyhow!("missing item snapshot {key}"))?;
            entries.push((key, entry));
        }
        entries.sort_by(|(left_id, left), (right_id, right)| {
            left.position
                .cmp(&right.position)
                .then_with(|| left_id.cmp(right_id))
        });
        Ok(entries)
    }

    pub fn item_texts(&self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .sorted_entries()?
            .into_iter()
            .map(|(_, entry)| entry.text)
            .collect())
    }

    fn scheme_items(&self) -> anyhow::Result<Vec<Item>> {
        self.sorted_entries()?
            .into_iter()
            .map(|(id, entry)| {
                serde_json::from_str::<Item>(&entry.snapshot_json)
                    .with_context(|| format!("parse item snapshot {id}"))
            })
            .collect()
    }
}

fn item_prelim(item: &Item, position: &str) -> anyhow::Result<MapPrelim> {
    Ok(MapPrelim::from([
        ("schema", In::from("knotq.item.v2")),
        ("id", In::from(item.id.to_string())),
        ("position", In::from(position.to_string())),
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

    fn apply_update_v1(&self, update: &[u8]) -> anyhow::Result<()> {
        self.doc
            .transact_mut()
            .apply_update(Update::decode_v1(update)?)?;
        Ok(())
    }

    fn validate(&self) -> anyhow::Result<()> {
        match self.kind {
            SyncDocumentKind::PersonalWorkspace => validate_workspace_document(&self.doc),
            SyncDocumentKind::Scheme => validate_scheme_document(&self.doc),
            SyncDocumentKind::Folder => Err(anyhow!("folder CRDT documents are not supported")),
        }
    }

    fn snapshot<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        let document = self.doc.get_or_insert_map("document");
        let txn = self.doc.transact();
        let json = document
            .get_as::<_, Option<String>>(&txn, "snapshot")?
            .ok_or_else(|| anyhow!("workspace snapshot missing"))?;
        Ok(serde_json::from_str(&json)?)
    }
}

#[derive(Deserialize)]
struct YrsSchemeItemSnapshot {
    schema: String,
    id: String,
    #[serde(default)]
    position: String,
    text: String,
    marker: String,
    indent: i64,
    start: String,
    end: String,
    available: String,
    media_json: String,
    repeats_json: String,
    state_json: String,
    priority_json: String,
    external_json: String,
    snapshot_json: String,
}

#[derive(Deserialize, Serialize)]
struct WorkspaceDocumentSnapshot {
    schema: String,
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

#[derive(Deserialize, Serialize)]
struct SchemeWorkspaceEntry {
    id: SchemeId,
    name: String,
    color_index: u8,
    gsync: bool,
    source: SchemeSource,
}

#[derive(Deserialize, Serialize)]
struct DailyQueueEntry {
    date: NaiveDate,
    scheme: SchemeId,
}

#[derive(Deserialize, Serialize)]
struct DeletedSchemeOriginEntry {
    scheme: SchemeId,
    origin: DeletedSchemeOrigin,
}

#[derive(Deserialize, Serialize)]
struct SchemeSyncEntry {
    scheme: SchemeId,
    sync: SyncDocumentMeta,
}

#[derive(Deserialize, Serialize)]
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
        schema: "knotq.workspace.v1".to_string(),
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

fn scheme_documents_by_id(workspace: &Workspace) -> HashMap<knotq_model::DocumentId, SchemeId> {
    workspace
        .scheme_sync
        .iter()
        .filter(|(_, meta)| meta.kind == SyncDocumentKind::Scheme)
        .map(|(scheme, meta)| (meta.id, *scheme))
        .collect()
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
    fn concurrent_content_edits_to_distinct_items_merge_without_duplicates() {
        let document = DocumentId::new();
        let mut base = Scheme::new("Plan", 0);
        base.items.push(Item::new("First"));
        base.items.push(Item::new("Second"));

        // Two replicas start from the same base state.
        let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
        let base_update = left.encode_update_v1(&[]).unwrap();
        let right = YrsSchemeDocument::new(document);
        right.apply_update_v1(&base_update).unwrap();

        // Each replica edits a *different* item's text concurrently.
        let mut scheme_left = base.clone();
        scheme_left.items[0].text = "First edited".to_string();
        let delta_left = left.sync_scheme(&scheme_left).unwrap().unwrap().update_v1;

        let mut scheme_right = base.clone();
        scheme_right.items[1].text = "Second edited".to_string();
        let delta_right = right.sync_scheme(&scheme_right).unwrap().unwrap().update_v1;

        // A third replica merges both concurrent deltas.
        let merged = YrsSchemeDocument::new(document);
        merged.apply_update_v1(&base_update).unwrap();
        merged.apply_update_v1(&delta_left).unwrap();
        merged.apply_update_v1(&delta_right).unwrap();

        // The order array is not rewritten on a content-only edit, so the merge
        // does not produce duplicate item_order entries and stays schema-valid.
        merged.validate().unwrap();
        assert_eq!(
            merged.item_texts().unwrap(),
            vec!["First edited", "Second edited"]
        );
    }

    #[test]
    fn crdt_schema_validation_accepts_workspace_snapshots() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Plan", 0);
        workspace.schemes.insert(scheme.id, scheme);
        workspace.ensure_sync_metadata();

        let mut docs = WorkspaceCrdtDocuments::empty(&workspace);
        let updates = docs
            .sync_changes(&workspace, &WorkspaceCrdtChangeSet::default().workspace())
            .updates;
        let workspace_updates = updates
            .iter()
            .filter(|update| update.kind == SyncDocumentKind::PersonalWorkspace)
            .map(|update| update.update_v1.as_slice());

        validate_crdt_update_sequence(SyncDocumentKind::PersonalWorkspace, workspace_updates)
            .unwrap();
    }

    #[test]
    fn crdt_schema_validation_accepts_scheme_history_and_delta() {
        let document = DocumentId::new();
        let mut scheme = Scheme::new("Plan", 0);
        scheme.items.push(Item::new("First"));
        let doc = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();
        let initial = doc.encode_update_v1(&[]).unwrap();

        scheme.items[0].text = "Changed".to_string();
        let delta = doc.sync_scheme(&scheme).unwrap().unwrap().update_v1;

        validate_crdt_update_sequence(
            SyncDocumentKind::Scheme,
            [initial.as_slice(), delta.as_slice()],
        )
        .unwrap();
    }

    #[test]
    fn crdt_schema_validation_rejects_malformed_update_bytes() {
        let err = validate_crdt_update_sequence(SyncDocumentKind::Scheme, [&[1, 2, 3][..]])
            .unwrap_err()
            .to_string();

        assert!(err.contains("decode update_v1"));
    }

    #[test]
    fn crdt_schema_validation_rejects_delta_without_base_document() {
        let document = DocumentId::new();
        let mut scheme = Scheme::new("Plan", 0);
        scheme.items.push(Item::new("First"));
        let doc = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();
        let _initial = doc.encode_update_v1(&[]).unwrap();

        scheme.items[0].text = "Changed".to_string();
        let delta = doc.sync_scheme(&scheme).unwrap().unwrap().update_v1;

        assert!(
            validate_crdt_update_sequence(SyncDocumentKind::Scheme, [delta.as_slice()]).is_err()
        );
    }

    #[test]
    fn crdt_schema_validation_rejects_bad_workspace_schema() {
        let doc = Doc::new();
        let document = doc.get_or_insert_map("document");
        let mut txn = doc.transact_mut();
        document.insert(
            &mut txn,
            "snapshot",
            serde_json::json!({
                "schema": "bad.workspace",
                "id": Workspace::new().id,
                "sync": {},
                "root": FolderId::new(),
                "folders": [],
                "schemes": [],
                "daily_queue": [],
                "recently_deleted": [],
                "deleted_scheme_origins": [],
                "scheme_sync": [],
                "folder_sync": []
            })
            .to_string(),
        );
        drop(txn);

        assert!(validate_crdt_update_sequence(
            SyncDocumentKind::PersonalWorkspace,
            [encode_full_update(&doc).as_slice()]
        )
        .is_err());
    }

    #[test]
    fn crdt_schema_validation_rejects_bad_scheme_schema() {
        let doc = valid_single_item_scheme_doc();
        let metadata = doc.get_or_insert_map("scheme_file");
        metadata.insert(&mut doc.transact_mut(), "schema", "bad.scheme");

        assert!(validate_crdt_update_sequence(
            SyncDocumentKind::Scheme,
            [encode_full_update(&doc).as_slice()]
        )
        .is_err());
    }

    #[test]
    fn crdt_schema_validation_rejects_item_without_position() {
        let doc = Doc::new();
        let metadata = doc.get_or_insert_map("scheme_file");
        let items_by_id = doc.get_or_insert_map("items_by_id");
        let item = Item::new("First");
        let mut txn = doc.transact_mut();
        metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V2);
        metadata.insert(&mut txn, "id", SchemeId::new().to_string());
        items_by_id.insert(
            &mut txn,
            item.id.to_string(),
            item_prelim(&item, "").unwrap(),
        );
        drop(txn);

        assert!(validate_crdt_update_sequence(
            SyncDocumentKind::Scheme,
            [encode_full_update(&doc).as_slice()]
        )
        .is_err());
    }

    #[test]
    fn crdt_schema_validation_rejects_item_id_key_mismatch() {
        let doc = Doc::new();
        let metadata = doc.get_or_insert_map("scheme_file");
        let items_by_id = doc.get_or_insert_map("items_by_id");
        let item = Item::new("First");
        let mut txn = doc.transact_mut();
        metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V2);
        metadata.insert(&mut txn, "id", SchemeId::new().to_string());
        // Store the item under a different (still valid) key than its own id.
        items_by_id.insert(
            &mut txn,
            ItemId::new().to_string(),
            item_prelim(&item, "V").unwrap(),
        );
        drop(txn);

        assert!(validate_crdt_update_sequence(
            SyncDocumentKind::Scheme,
            [encode_full_update(&doc).as_slice()]
        )
        .is_err());
    }

    #[test]
    fn concurrent_inserts_into_same_gap_merge_without_wedge() {
        let document = DocumentId::new();
        let mut base = Scheme::new("Plan", 0);
        base.items.push(Item::new("A"));
        base.items.push(Item::new("B"));

        let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
        let base_update = left.encode_update_v1(&[]).unwrap();
        let right = YrsSchemeDocument::new(document);
        right.apply_update_v1(&base_update).unwrap();

        // Both replicas insert a new item into the *same* gap (between A and B)
        // offline, so they independently generate the same fractional position.
        let mut left_scheme = base.clone();
        left_scheme.items.insert(1, Item::new("X"));
        let delta_left = left.sync_scheme(&left_scheme).unwrap().unwrap().update_v1;

        let mut right_scheme = base.clone();
        right_scheme.items.insert(1, Item::new("Y"));
        let delta_right = right.sync_scheme(&right_scheme).unwrap().unwrap().update_v1;

        let merged = YrsSchemeDocument::new(document);
        merged.apply_update_v1(&base_update).unwrap();
        merged.apply_update_v1(&delta_left).unwrap();
        merged.apply_update_v1(&delta_right).unwrap();

        // Identical positions are fine: the id tiebreak keeps a deterministic
        // total order, both inserts survive, and the schema stays valid.
        merged.validate().unwrap();
        let texts = merged.item_texts().unwrap();
        assert_eq!(texts.len(), 4, "{texts:?}");
        assert_eq!(texts[0], "A");
        assert_eq!(texts[3], "B");
        assert!(texts.contains(&"X".to_string()));
        assert!(texts.contains(&"Y".to_string()));
    }

    #[test]
    fn crdt_schema_validation_rejects_folder_documents() {
        let doc = Doc::new();
        assert!(validate_crdt_update_sequence(
            SyncDocumentKind::Folder,
            [encode_full_update(&doc).as_slice()]
        )
        .is_err());
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
    fn remote_crdt_updates_materialize_workspace_and_scheme_items() {
        let mut source = Workspace::new();
        let mut scheme = Scheme::new("Remote Plan", 2);
        scheme.items.push(Item::new("First remote line"));
        let scheme_id = scheme.id;
        source
            .folders
            .get_mut(&source.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));
        source.schemes.insert(scheme_id, scheme);
        source.ensure_sync_metadata();

        let updates = WorkspaceCrdtDocuments::snapshot_updates(&source)
            .updates
            .into_iter()
            .enumerate()
            .map(|(index, update)| StoredCrdtUpdate {
                workspace_id: source.id,
                document: update.document,
                kind: update.kind,
                replica_id: knotq_model::ReplicaId::new(),
                sequence: (index + 1) as u64,
                received_at: chrono::Utc::now(),
                update_v1: update.update_v1,
            })
            .collect::<Vec<_>>();

        let mut target = source.clone();
        target.schemes.get_mut(&scheme_id).unwrap().items.clear();
        let mut docs = WorkspaceCrdtDocuments::try_new(&target).unwrap();
        let outcome = docs.apply_remote_updates(&target, &updates);

        assert!(outcome.is_ok(), "{:?}", outcome.errors);
        assert_eq!(
            outcome.workspace.schemes[&scheme_id].items[0].text,
            "First remote line"
        );
        assert!(outcome.workspace.folders[&outcome.workspace.root]
            .children
            .contains(&NodeRef::Scheme(scheme_id)));
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

    fn valid_single_item_scheme_doc() -> Doc {
        let doc = Doc::new();
        let metadata = doc.get_or_insert_map("scheme_file");
        let items_by_id = doc.get_or_insert_map("items_by_id");
        let scheme = Scheme::new("Plan", 0);
        let item = Item::new("First");
        let item_id = item.id.to_string();
        let mut txn = doc.transact_mut();
        metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V2);
        metadata.insert(&mut txn, "id", scheme.id.to_string());
        items_by_id.insert(&mut txn, item_id, item_prelim(&item, "V").unwrap());
        drop(txn);
        doc
    }

    fn encode_full_update(doc: &Doc) -> Vec<u8> {
        doc.transact().encode_diff_v1(&StateVector::default())
    }
}
