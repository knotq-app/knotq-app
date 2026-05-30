use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context};
use chrono::{DateTime, NaiveDate};
use knotq_model::{
    DeletedSchemeOrigin, DocumentId, Folder, FolderId, Item, ItemId, Scheme, SchemeId,
    NodeRef, SchemeSource, SyncDocumentKind, SyncDocumentMeta, Workspace,
};
use serde::{Deserialize, Serialize};
use yrs::updates::{decoder::Decode, encoder::Encode};
use yrs::{
    Doc, GetString, In, Map, MapPrelim, MapRef, OffsetKind, Options, Out, ReadTxn, StateVector,
    Text, TextPrelim, TextRef, Transact, TransactionMut, Update,
};

use crate::{CrdtDocumentUpdate, StoredCrdtUpdate};

const SCHEME_SCHEMA_V1: &str = "knotq.scheme_file.v1";
const WORKSPACE_SCHEMA_V1: &str = "knotq.workspace.v1";

const NODE_KIND_FOLDER: &str = "folder";
const NODE_KIND_SCHEME: &str = "scheme";

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
    let meta = doc.get_or_insert_map("meta");
    let nodes = doc.get_or_insert_map("nodes");
    let txn = doc.transact();

    let schema = meta
        .get_as::<_, Option<String>>(&txn, "schema")
        .context("read workspace schema")?
        .ok_or_else(|| anyhow!("workspace schema missing"))?;
    if schema != WORKSPACE_SCHEMA_V1 {
        return Err(anyhow!("workspace schema invalid"));
    }
    let id = meta
        .get_as::<_, Option<String>>(&txn, "id")
        .context("read workspace id")?
        .ok_or_else(|| anyhow!("workspace id missing"))?;
    id.parse::<uuid::Uuid>().context("workspace id invalid")?;
    let root = meta
        .get_as::<_, Option<String>>(&txn, "root")
        .context("read workspace root")?
        .ok_or_else(|| anyhow!("workspace root missing"))?;
    root.parse::<uuid::Uuid>().context("workspace root invalid")?;
    let sync = meta
        .get_as::<_, Option<String>>(&txn, "sync")
        .context("read workspace sync")?
        .ok_or_else(|| anyhow!("workspace sync missing"))?;
    let sync: serde_json::Value =
        serde_json::from_str(&sync).context("workspace sync is not JSON")?;
    if !sync.is_object() {
        return Err(anyhow!("workspace sync is not an object"));
    }

    // Folders and schemes are stored as individual, id-keyed entries so that
    // concurrent additions on different replicas merge instead of resolving as a
    // single whole-document last-writer-wins.
    for key in nodes.keys(&txn).map(str::to_string).collect::<Vec<_>>() {
        let json = nodes
            .get_as::<_, Option<String>>(&txn, &key)
            .with_context(|| format!("read node {key}"))?
            .ok_or_else(|| anyhow!("node entry missing: {key}"))?;
        let entry: WorkspaceNodeEntry =
            serde_json::from_str(&json).with_context(|| format!("node invalid: {key}"))?;
        if entry.id != key {
            return Err(anyhow!("node id mismatch: {key}"));
        }
        key.parse::<uuid::Uuid>()
            .with_context(|| format!("node id invalid: {key}"))?;
        if entry.kind != NODE_KIND_FOLDER && entry.kind != NODE_KIND_SCHEME {
            return Err(anyhow!("node kind invalid: {key}"));
        }
        if entry.position.is_empty() {
            return Err(anyhow!("node position missing: {key}"));
        }
        if !entry.parent.is_empty() {
            entry
                .parent
                .parse::<uuid::Uuid>()
                .with_context(|| format!("node parent invalid: {key}"))?;
        }
        serde_json::from_str::<serde_json::Value>(&entry.payload)
            .with_context(|| format!("node payload invalid: {key}"))?;
    }

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
    if schema != SCHEME_SCHEMA_V1 {
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
        let item_map = match items_by_id.get(&txn, &item_id) {
            Some(Out::YMap(map)) => map,
            _ => return Err(anyhow!("item entry missing or not a map: {item_id}")),
        };
        validate_scheme_item(&item_id, parsed_item_id, &item_map, &txn)?;
    }

    Ok(())
}

fn validate_scheme_item(
    item_id: &str,
    parsed_item_id: ItemId,
    item_map: &MapRef,
    txn: &impl ReadTxn,
) -> anyhow::Result<()> {
    let str_field = |key: &str| item_map.get_as::<_, Option<String>>(txn, key).ok().flatten();
    let require_str = |key: &str| {
        str_field(key).ok_or_else(|| anyhow!("item {key} missing: {item_id}"))
    };

    if require_str("schema")? != "knotq.item.v1" {
        return Err(anyhow!("item schema invalid: {item_id}"));
    }
    let id = require_str("id")?;
    if id != item_id {
        return Err(anyhow!("item id mismatch: {item_id}"));
    }
    if require_str("position")?.is_empty() {
        return Err(anyhow!("item position missing: {item_id}"));
    }
    if !matches!(
        require_str("marker")?.as_str(),
        "blank" | "bullet" | "numbered" | "checkbox"
    ) {
        return Err(anyhow!("item marker invalid: {item_id}"));
    }
    let indent = item_map
        .get_as::<_, Option<i64>>(txn, "indent")
        .ok()
        .flatten()
        .ok_or_else(|| anyhow!("item indent missing: {item_id}"))?;
    if !(0..=i64::from(u8::MAX)).contains(&indent) {
        return Err(anyhow!("item indent invalid: {item_id}"));
    }
    parse_optional_rfc3339(&require_str("start")?)
        .with_context(|| format!("item start invalid: {item_id}"))?;
    parse_optional_rfc3339(&require_str("end")?)
        .with_context(|| format!("item end invalid: {item_id}"))?;
    parse_optional_rfc3339(&require_str("available")?)
        .with_context(|| format!("item available invalid: {item_id}"))?;
    parse_json_value(&require_str("media_json")?)
        .with_context(|| format!("item media invalid: {item_id}"))?;
    parse_json_value(&require_str("repeats_json")?)
        .with_context(|| format!("item repeats invalid: {item_id}"))?;
    parse_json_value(&require_str("state_json")?)
        .with_context(|| format!("item state invalid: {item_id}"))?;
    parse_json_value(&require_str("priority_json")?)
        .with_context(|| format!("item priority invalid: {item_id}"))?;
    parse_json_value(&require_str("external_json")?)
        .with_context(|| format!("item external invalid: {item_id}"))?;

    // Text is a collaborative sequence CRDT (yrs Text), not a plain string, so
    // concurrent character edits merge. Its presence (and that it reads as text)
    // is the structural requirement; any content is valid line text.
    if item_text_ref(item_map, txn).is_none() {
        return Err(anyhow!("item text missing or not a text type: {item_id}"));
    }

    // snapshot_json carries every non-text field for materialization; text lives
    // in the Text CRDT and is intentionally absent here.
    let snapshot: Item = serde_json::from_str(&require_str("snapshot_json")?)
        .with_context(|| format!("item snapshot invalid: {item_id}"))?;
    if snapshot.id != parsed_item_id {
        return Err(anyhow!("item snapshot id mismatch: {item_id}"));
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
        // UTF-16 offsets match Yjs (JS) semantics, so the text-diff index math
        // here lines up with any future JavaScript collaboration client and never
        // splits a multi-byte character.
        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
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
            != Some(SCHEME_SCHEMA_V1)
        {
            metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
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
        let mut stored: HashMap<String, StoredItem> = HashMap::new();
        for key in stored_keys {
            if let Some(item_map) = item_map_ref(&items_by_id, &txn, &key) {
                stored.insert(key, read_stored_item(&item_map, &txn));
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

        // For each item, merge text as a sequence CRDT and treat the rest as
        // last-writer-wins metadata:
        //   - new item        -> insert the full entry (text seeded into a Text type)
        //   - text changed     -> apply a minimal insert/delete diff to the Text so
        //                         concurrent character edits converge
        //   - metadata changed -> rewrite the scalar fields + snapshot blob only
        // so a text edit never recreates (and clobbers) the collaborative Text.
        for (item, position) in scheme.items.iter().zip(&positions) {
            let item_id = item.id.to_string();
            let next_snapshot = item_snapshot_json(item)?;
            let prev = stored.get(&item_id);
            match item_map_ref(&items_by_id, &txn, &item_id) {
                None => {
                    items_by_id.insert(&mut txn, item_id, item_prelim(item, position)?);
                }
                Some(item_map) => {
                    match item_text_ref(&item_map, &txn) {
                        Some(text_ref) => {
                            let current = match prev {
                                Some(stored) => stored.text.clone(),
                                None => text_ref.get_string(&txn),
                            };
                            if current != item.text {
                                apply_text_diff(&text_ref, &mut txn, &current, &item.text);
                            }
                        }
                        // No Text present (corrupt/legacy entry) — rebuild it whole.
                        None => {
                            items_by_id.insert(&mut txn, item_id, item_prelim(item, position)?);
                            continue;
                        }
                    }
                    let metadata_changed = prev.is_none_or(|stored| {
                        stored.snapshot_json != next_snapshot || stored.position != *position
                    });
                    if metadata_changed {
                        write_item_metadata(&item_map, &mut txn, item, position, &next_snapshot)?;
                    }
                }
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
    fn sorted_entries(&self) -> anyhow::Result<Vec<(String, StoredItem)>> {
        let items_by_id = self.doc.get_or_insert_map("items_by_id");
        let txn = self.doc.transact();
        let keys = items_by_id
            .keys(&txn)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut entries = Vec::with_capacity(keys.len());
        for key in keys {
            let item_map = item_map_ref(&items_by_id, &txn, &key)
                .ok_or_else(|| anyhow!("missing item {key}"))?;
            entries.push((key, read_stored_item(&item_map, &txn)));
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
                // snapshot_json holds every field except text; text comes from the
                // Text CRDT, which is the source of truth for line content.
                let mut item: Item = serde_json::from_str(&entry.snapshot_json)
                    .with_context(|| format!("parse item snapshot {id}"))?;
                item.text = entry.text;
                Ok(item)
            })
            .collect()
    }
}

/// What we need from a stored item entry without deserializing the whole nested
/// map (its `text` is a Text type, not a scalar serde can read).
struct StoredItem {
    position: String,
    snapshot_json: String,
    text: String,
}

fn item_map_ref(items_by_id: &MapRef, txn: &impl ReadTxn, key: &str) -> Option<MapRef> {
    match items_by_id.get(txn, key) {
        Some(Out::YMap(map)) => Some(map),
        _ => None,
    }
}

fn item_text_ref(item_map: &MapRef, txn: &impl ReadTxn) -> Option<TextRef> {
    match item_map.get(txn, "text") {
        Some(Out::YText(text)) => Some(text),
        _ => None,
    }
}

fn read_stored_item(item_map: &MapRef, txn: &impl ReadTxn) -> StoredItem {
    let str_field = |key: &str| {
        item_map
            .get_as::<_, Option<String>>(txn, key)
            .ok()
            .flatten()
            .unwrap_or_default()
    };
    StoredItem {
        position: str_field("position"),
        snapshot_json: str_field("snapshot_json"),
        text: item_text_ref(item_map, txn)
            .map(|text| text.get_string(txn))
            .unwrap_or_default(),
    }
}

/// Serialize every item field except text. Text is owned by the Text CRDT, so
/// keeping it out of the snapshot blob means a text edit never rewrites the blob
/// and the two representations cannot disagree.
fn item_snapshot_json(item: &Item) -> anyhow::Result<String> {
    let mut snapshot = item.clone();
    snapshot.text = String::new();
    Ok(serde_json::to_string(&snapshot)?)
}

/// Rewrite an existing item's last-writer-wins metadata fields in place, leaving
/// its collaborative Text untouched.
fn write_item_metadata(
    item_map: &MapRef,
    txn: &mut TransactionMut,
    item: &Item,
    position: &str,
    snapshot_json: &str,
) -> anyhow::Result<()> {
    item_map.insert(txn, "schema", "knotq.item.v1");
    item_map.insert(txn, "id", item.id.to_string());
    item_map.insert(txn, "position", position.to_string());
    item_map.insert(txn, "marker", serde_json_string_value(&item.marker)?);
    item_map.insert(txn, "indent", i64::from(item.indent));
    item_map.insert(
        txn,
        "start",
        item.start.map(|dt| dt.to_rfc3339()).unwrap_or_default(),
    );
    item_map.insert(
        txn,
        "end",
        item.end.map(|dt| dt.to_rfc3339()).unwrap_or_default(),
    );
    item_map.insert(
        txn,
        "available",
        item.available.map(|dt| dt.to_rfc3339()).unwrap_or_default(),
    );
    item_map.insert(txn, "media_json", serde_json::to_string(&item.media)?);
    item_map.insert(txn, "repeats_json", serde_json::to_string(&item.repeats)?);
    item_map.insert(txn, "state_json", serde_json::to_string(&item.state)?);
    item_map.insert(txn, "priority_json", serde_json::to_string(&item.priority)?);
    item_map.insert(txn, "external_json", serde_json::to_string(&item.external)?);
    item_map.insert(txn, "snapshot_json", snapshot_json.to_string());
    Ok(())
}

/// Apply the change from `old` to `new` as a single contiguous splice on the Text
/// (the common prefix and suffix are left untouched), so a typical edit becomes a
/// minimal insert/delete that merges character-for-character under concurrency.
/// Offsets are UTF-16 code units to match the doc's OffsetKind and Yjs.
fn apply_text_diff(text: &TextRef, txn: &mut TransactionMut, old: &str, new: &str) {
    if old == new {
        return;
    }
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let min_len = old_chars.len().min(new_chars.len());
    let mut prefix = 0;
    while prefix < min_len && old_chars[prefix] == new_chars[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < (min_len - prefix)
        && old_chars[old_chars.len() - 1 - suffix] == new_chars[new_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let utf16_len = |chars: &[char]| chars.iter().map(|c| c.len_utf16() as u32).sum::<u32>();
    let at = utf16_len(&old_chars[..prefix]);
    let removed = utf16_len(&old_chars[prefix..old_chars.len() - suffix]);
    let inserted: String = new_chars[prefix..new_chars.len() - suffix].iter().collect();
    if removed > 0 {
        text.remove_range(txn, at, removed);
    }
    if !inserted.is_empty() {
        text.insert(txn, at, &inserted);
    }
}

fn item_prelim(item: &Item, position: &str) -> anyhow::Result<MapPrelim> {
    Ok(MapPrelim::from([
        ("schema", In::from("knotq.item.v1")),
        ("id", In::from(item.id.to_string())),
        ("position", In::from(position.to_string())),
        // Collaborative line text as a sequence CRDT (merges character edits).
        ("text", In::from(TextPrelim::new(item.text.clone()))),
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
        ("snapshot_json", In::from(item_snapshot_json(item)?)),
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
        // The workspace document is decomposed into independent, id-keyed maps so
        // that concurrent edits to distinct entities (e.g. two replicas each adding
        // a folder) merge additively instead of resolving as whole-document LWW.
        doc.get_or_insert_map("meta");
        doc.get_or_insert_map("nodes");
        doc.get_or_insert_map("scheme_sync");
        doc.get_or_insert_map("folder_sync");
        doc.get_or_insert_map("daily_queue");
        doc.get_or_insert_map("recently_deleted");
        doc.get_or_insert_map("deleted_scheme_origins");
        Self { id, kind, doc }
    }

    fn sync_snapshot(
        &self,
        snapshot: &WorkspaceDocumentSnapshot,
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

    fn replace_snapshot(&self, snapshot: &WorkspaceDocumentSnapshot) -> anyhow::Result<bool> {
        let meta = self.doc.get_or_insert_map("meta");
        let nodes = self.doc.get_or_insert_map("nodes");
        let scheme_sync = self.doc.get_or_insert_map("scheme_sync");
        let folder_sync = self.doc.get_or_insert_map("folder_sync");
        let daily_queue = self.doc.get_or_insert_map("daily_queue");
        let recently_deleted = self.doc.get_or_insert_map("recently_deleted");
        let deleted_origins = self.doc.get_or_insert_map("deleted_scheme_origins");
        let mut txn = self.doc.transact_mut();

        // Reuse positions already stored so an unchanged tree re-serializes to
        // byte-identical entries, producing no update.
        let stored_node_positions = node_positions(&nodes, &txn);
        let stored_deleted_positions = string_map_entries(&recently_deleted, &txn)
            .into_iter()
            .collect::<HashMap<_, _>>();

        // Derive each node's parent and sibling order from the authoritative
        // folder.children lists, then assign fractional positions per parent group
        // so concurrent inserts/reorders merge without a duplicate-id wedge.
        let mut membership_parent: HashMap<String, String> = HashMap::new();
        let mut children_by_parent: HashMap<String, Vec<String>> = HashMap::new();
        for folder in &snapshot.folders {
            let parent = folder.id.to_string();
            for child in &folder.children {
                let child_id = node_ref_id(child);
                membership_parent.insert(child_id.clone(), parent.clone());
                children_by_parent
                    .entry(parent.clone())
                    .or_default()
                    .push(child_id);
            }
        }
        let mut positions: HashMap<String, String> = HashMap::new();
        for ordered in children_by_parent.values() {
            assign_fractional_positions(ordered, &stored_node_positions, &mut positions);
        }
        // The root folder (and any orphan) is nobody's child; give it a stable
        // standalone key so every node carries a non-empty position.
        let ensure_position = |id: &str, positions: &mut HashMap<String, String>| {
            if !positions.contains_key(id) {
                let position = stored_node_positions
                    .get(id)
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .unwrap_or_else(|| crate::fractional::between(None, None));
                positions.insert(id.to_string(), position);
            }
        };

        let mut node_entries: Vec<(String, String)> = Vec::new();
        for folder in &snapshot.folders {
            let id = folder.id.to_string();
            ensure_position(&id, &mut positions);
            let payload = serde_json::to_string(&FolderPayload {
                name: folder.name.clone(),
                expanded: folder.expanded,
                parent: folder.parent,
            })?;
            node_entries.push((
                id.clone(),
                node_entry_json(&id, NODE_KIND_FOLDER, &membership_parent, &positions, payload)?,
            ));
        }
        for scheme in &snapshot.schemes {
            let id = scheme.id.to_string();
            ensure_position(&id, &mut positions);
            let payload = serde_json::to_string(scheme)?;
            node_entries.push((
                id.clone(),
                node_entry_json(&id, NODE_KIND_SCHEME, &membership_parent, &positions, payload)?,
            ));
        }

        // recently_deleted is order-bearing, so position it the same way.
        let deleted_ids = snapshot
            .recently_deleted
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
        let mut deleted_positions: HashMap<String, String> = HashMap::new();
        assign_fractional_positions(
            &deleted_ids,
            &stored_deleted_positions,
            &mut deleted_positions,
        );
        let recently_deleted_entries = deleted_ids
            .iter()
            .map(|id| (id.clone(), deleted_positions.get(id).cloned().unwrap_or_default()))
            .collect::<Vec<_>>();

        let mut scheme_sync_entries = Vec::with_capacity(snapshot.scheme_sync.len());
        for entry in &snapshot.scheme_sync {
            scheme_sync_entries.push((entry.scheme.to_string(), serde_json::to_string(&entry.sync)?));
        }
        let mut folder_sync_entries = Vec::with_capacity(snapshot.folder_sync.len());
        for entry in &snapshot.folder_sync {
            folder_sync_entries.push((entry.folder.to_string(), serde_json::to_string(&entry.sync)?));
        }
        let mut daily_queue_entries = Vec::with_capacity(snapshot.daily_queue.len());
        for entry in &snapshot.daily_queue {
            daily_queue_entries.push((entry.date.to_string(), entry.scheme.to_string()));
        }
        let mut deleted_origin_entries = Vec::with_capacity(snapshot.deleted_scheme_origins.len());
        for entry in &snapshot.deleted_scheme_origins {
            deleted_origin_entries
                .push((entry.scheme.to_string(), serde_json::to_string(&entry.origin)?));
        }

        let mut changed = false;
        changed |= sync_string_map(
            &meta,
            &mut txn,
            &[
                ("schema".to_string(), WORKSPACE_SCHEMA_V1.to_string()),
                ("id".to_string(), snapshot.id.to_string()),
                ("root".to_string(), snapshot.root.to_string()),
                ("sync".to_string(), serde_json::to_string(&snapshot.sync)?),
            ],
        );
        changed |= sync_string_map(&nodes, &mut txn, &node_entries);
        changed |= sync_string_map(&scheme_sync, &mut txn, &scheme_sync_entries);
        changed |= sync_string_map(&folder_sync, &mut txn, &folder_sync_entries);
        changed |= sync_string_map(&daily_queue, &mut txn, &daily_queue_entries);
        changed |= sync_string_map(&recently_deleted, &mut txn, &recently_deleted_entries);
        changed |= sync_string_map(&deleted_origins, &mut txn, &deleted_origin_entries);
        Ok(changed)
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

    fn snapshot(&self) -> anyhow::Result<WorkspaceDocumentSnapshot> {
        let meta = self.doc.get_or_insert_map("meta");
        let nodes = self.doc.get_or_insert_map("nodes");
        let scheme_sync_map = self.doc.get_or_insert_map("scheme_sync");
        let folder_sync_map = self.doc.get_or_insert_map("folder_sync");
        let daily_queue_map = self.doc.get_or_insert_map("daily_queue");
        let recently_deleted_map = self.doc.get_or_insert_map("recently_deleted");
        let deleted_origins_map = self.doc.get_or_insert_map("deleted_scheme_origins");
        let txn = self.doc.transact();

        let read_meta = |key: &str| -> anyhow::Result<String> {
            meta.get_as::<_, Option<String>>(&txn, key)
                .with_context(|| format!("read workspace {key}"))?
                .ok_or_else(|| anyhow!("workspace {key} missing"))
        };
        let id = read_meta("id")?.parse().context("workspace id invalid")?;
        let root: FolderId = read_meta("root")?.parse().context("workspace root invalid")?;
        let sync: SyncDocumentMeta =
            serde_json::from_str(&read_meta("sync")?).context("workspace sync invalid")?;

        struct ParsedNode {
            kind: String,
            parent: String,
            position: String,
            payload: String,
        }
        let mut parsed: HashMap<String, ParsedNode> = HashMap::new();
        let mut folder_ids: HashSet<String> = HashSet::new();
        for (key, value) in string_map_entries(&nodes, &txn) {
            let entry: WorkspaceNodeEntry =
                serde_json::from_str(&value).with_context(|| format!("node invalid: {key}"))?;
            if entry.kind == NODE_KIND_FOLDER {
                folder_ids.insert(key.clone());
            }
            parsed.insert(
                key,
                ParsedNode {
                    kind: entry.kind,
                    parent: entry.parent,
                    position: entry.position,
                    payload: entry.payload,
                },
            );
        }

        let root_key = root.to_string();
        // Each node's effective parent is an existing folder, else the root —
        // orphans re-home under root rather than vanishing.
        let mut children_by_parent: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (id_str, node) in &parsed {
            if *id_str == root_key {
                continue;
            }
            let parent = if !node.parent.is_empty() && folder_ids.contains(&node.parent) {
                node.parent.clone()
            } else {
                root_key.clone()
            };
            children_by_parent
                .entry(parent)
                .or_default()
                .push((node.position.clone(), id_str.clone()));
        }
        for children in children_by_parent.values_mut() {
            children.sort_by(|(lp, lid), (rp, rid)| lp.cmp(rp).then_with(|| lid.cmp(rid)));
        }

        let node_ref_for = |id_str: &str| -> anyhow::Result<NodeRef> {
            if folder_ids.contains(id_str) {
                Ok(NodeRef::Folder(
                    id_str
                        .parse()
                        .with_context(|| format!("folder id invalid: {id_str}"))?,
                ))
            } else {
                Ok(NodeRef::Scheme(
                    id_str
                        .parse()
                        .with_context(|| format!("scheme id invalid: {id_str}"))?,
                ))
            }
        };

        let mut folders = Vec::new();
        let mut schemes = Vec::new();
        for (id_str, node) in &parsed {
            if node.kind == NODE_KIND_FOLDER {
                let payload: FolderPayload = serde_json::from_str(&node.payload)
                    .with_context(|| format!("folder payload invalid: {id_str}"))?;
                let children = children_by_parent
                    .get(id_str)
                    .map(|kids| {
                        kids.iter()
                            .map(|(_, child_id)| node_ref_for(child_id))
                            .collect::<anyhow::Result<Vec<_>>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                folders.push(Folder {
                    id: id_str
                        .parse()
                        .with_context(|| format!("folder id invalid: {id_str}"))?,
                    name: payload.name,
                    parent: payload.parent,
                    children,
                    expanded: payload.expanded,
                });
            } else {
                let entry: SchemeWorkspaceEntry = serde_json::from_str(&node.payload)
                    .with_context(|| format!("scheme payload invalid: {id_str}"))?;
                schemes.push(entry);
            }
        }
        folders.sort_by_key(|folder| folder.id.to_string());
        schemes.sort_by_key(|scheme| scheme.id.to_string());

        let mut deleted = string_map_entries(&recently_deleted_map, &txn)
            .into_iter()
            .map(|(id, position)| {
                let scheme = id
                    .parse::<SchemeId>()
                    .with_context(|| format!("recently deleted id invalid: {id}"))?;
                Ok::<_, anyhow::Error>((position, id, scheme))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        deleted.sort_by(|(lp, lid, _), (rp, rid, _)| lp.cmp(rp).then_with(|| lid.cmp(rid)));
        let recently_deleted = deleted.into_iter().map(|(_, _, scheme)| scheme).collect();

        let mut daily_queue = string_map_entries(&daily_queue_map, &txn)
            .into_iter()
            .map(|(date, scheme)| {
                Ok::<_, anyhow::Error>(DailyQueueEntry {
                    date: date
                        .parse()
                        .with_context(|| format!("daily queue date invalid: {date}"))?,
                    scheme: scheme
                        .parse()
                        .with_context(|| format!("daily queue scheme invalid: {scheme}"))?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        daily_queue.sort_by_key(|entry| entry.date);

        let mut deleted_scheme_origins = string_map_entries(&deleted_origins_map, &txn)
            .into_iter()
            .map(|(scheme, origin)| {
                Ok::<_, anyhow::Error>(DeletedSchemeOriginEntry {
                    scheme: scheme
                        .parse()
                        .with_context(|| format!("deleted origin scheme invalid: {scheme}"))?,
                    origin: serde_json::from_str(&origin)
                        .with_context(|| format!("deleted origin invalid: {scheme}"))?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        deleted_scheme_origins.sort_by_key(|entry| entry.scheme.to_string());

        let mut scheme_sync = string_map_entries(&scheme_sync_map, &txn)
            .into_iter()
            .map(|(scheme, sync)| {
                Ok::<_, anyhow::Error>(SchemeSyncEntry {
                    scheme: scheme
                        .parse()
                        .with_context(|| format!("scheme sync id invalid: {scheme}"))?,
                    sync: serde_json::from_str(&sync)
                        .with_context(|| format!("scheme sync invalid: {scheme}"))?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        scheme_sync.sort_by_key(|entry| entry.scheme.to_string());

        let mut folder_sync = string_map_entries(&folder_sync_map, &txn)
            .into_iter()
            .map(|(folder, sync)| {
                Ok::<_, anyhow::Error>(FolderSyncEntry {
                    folder: folder
                        .parse()
                        .with_context(|| format!("folder sync id invalid: {folder}"))?,
                    sync: serde_json::from_str(&sync)
                        .with_context(|| format!("folder sync invalid: {folder}"))?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        folder_sync.sort_by_key(|entry| entry.folder.to_string());

        Ok(WorkspaceDocumentSnapshot {
            schema: WORKSPACE_SCHEMA_V1.to_string(),
            id,
            sync,
            root,
            folders,
            schemes,
            daily_queue,
            recently_deleted,
            deleted_scheme_origins,
            scheme_sync,
            folder_sync,
        })
    }
}

/// One folder or scheme stored as an individual, id-keyed entry in the workspace
/// document's `nodes` map. `parent`/`position` carry the tree structure so that
/// it can be reconstructed (and merged) without a shared, wedge-prone array.
#[derive(Serialize, Deserialize)]
struct WorkspaceNodeEntry {
    id: String,
    kind: String,
    #[serde(default)]
    parent: String,
    #[serde(default)]
    position: String,
    payload: String,
}

#[derive(Serialize, Deserialize)]
struct FolderPayload {
    name: String,
    expanded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<FolderId>,
}

fn node_ref_id(node: &NodeRef) -> String {
    match node {
        NodeRef::Folder(id) => id.to_string(),
        NodeRef::Scheme(id) => id.to_string(),
    }
}

fn node_entry_json(
    id: &str,
    kind: &str,
    membership_parent: &HashMap<String, String>,
    positions: &HashMap<String, String>,
    payload: String,
) -> anyhow::Result<String> {
    let entry = WorkspaceNodeEntry {
        id: id.to_string(),
        kind: kind.to_string(),
        parent: membership_parent.get(id).cloned().unwrap_or_default(),
        position: positions.get(id).cloned().unwrap_or_default(),
        payload,
    };
    Ok(serde_json::to_string(&entry)?)
}

/// Positions currently stored per node id, used to keep keys stable across syncs.
fn node_positions(map: &MapRef, txn: &impl ReadTxn) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (key, value) in string_map_entries(map, txn) {
        if let Ok(entry) = serde_json::from_str::<WorkspaceNodeEntry>(&value) {
            out.insert(key, entry.position);
        }
    }
    out
}

fn string_map_entries(map: &MapRef, txn: &impl ReadTxn) -> Vec<(String, String)> {
    let keys = map.keys(txn).map(str::to_string).collect::<Vec<_>>();
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        if let Ok(Some(value)) = map.get_as::<_, Option<String>>(txn, &key) {
            out.push((key, value));
        }
    }
    out
}

/// Reconcile a string→string map to `desired`: remove keys no longer present and
/// (re)insert only entries whose value changed, so a single edit yields a single
/// map-entry delta. Returns whether anything changed.
fn sync_string_map(map: &MapRef, txn: &mut TransactionMut, desired: &[(String, String)]) -> bool {
    let mut changed = false;
    let desired_keys: HashSet<&str> = desired.iter().map(|(key, _)| key.as_str()).collect();
    let stale = map
        .keys(&*txn)
        .filter(|key| !desired_keys.contains(*key))
        .map(str::to_string)
        .collect::<Vec<_>>();
    for key in stale {
        map.remove(&mut *txn, &key);
        changed = true;
    }
    for (key, value) in desired {
        let existing = map.get_as::<_, Option<String>>(&*txn, key).ok().flatten();
        if existing.as_deref() != Some(value.as_str()) {
            map.insert(&mut *txn, key.clone(), value.clone());
            changed = true;
        }
    }
    changed
}

/// Assign each id in `ordered` a fractional key, keeping an existing key whenever
/// it still sorts after the previous one; otherwise mint a fresh key between
/// neighbors. Identical concurrent keys are harmless: callers break ties on id.
fn assign_fractional_positions(
    ordered: &[String],
    stored: &HashMap<String, String>,
    out: &mut HashMap<String, String>,
) {
    let mut prev: Option<String> = None;
    for (idx, id) in ordered.iter().enumerate() {
        let existing = stored.get(id).filter(|value| !value.is_empty()).cloned();
        let keep = match (&existing, &prev) {
            (Some(existing), Some(prev)) => existing.as_str() > prev.as_str(),
            (Some(_), None) => true,
            (None, _) => false,
        };
        let position = if keep {
            existing.unwrap()
        } else {
            let upper = ordered[idx + 1..].iter().find_map(|next| {
                stored
                    .get(next)
                    .filter(|candidate| {
                        !candidate.is_empty()
                            && prev.as_deref().is_none_or(|prev| candidate.as_str() > prev)
                    })
                    .cloned()
            });
            crate::fractional::between(prev.as_deref(), upper.as_deref())
        };
        prev = Some(position.clone());
        out.insert(id.clone(), position);
    }
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
        schema: WORKSPACE_SCHEMA_V1.to_string(),
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
    fn concurrent_edits_to_same_item_text_merge_character_wise() {
        let document = DocumentId::new();
        let mut base = Scheme::new("Plan", 0);
        base.items.push(Item::new("hello"));

        // Two replicas start from the same single-line base.
        let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
        let base_update = left.encode_update_v1(&[]).unwrap();
        let right = YrsSchemeDocument::new(document);
        right.apply_update_v1(&base_update).unwrap();

        // Both edit the *same* line concurrently: left appends, right prepends.
        let mut scheme_left = base.clone();
        scheme_left.items[0].text = "hello!".to_string();
        let delta_left = left.sync_scheme(&scheme_left).unwrap().unwrap().update_v1;

        let mut scheme_right = base.clone();
        scheme_right.items[0].text = "Xhello".to_string();
        let delta_right = right.sync_scheme(&scheme_right).unwrap().unwrap().update_v1;

        // Merge both concurrent edits into a third replica.
        let merged = YrsSchemeDocument::new(document);
        merged.apply_update_v1(&base_update).unwrap();
        merged.apply_update_v1(&delta_left).unwrap();
        merged.apply_update_v1(&delta_right).unwrap();

        merged.validate().unwrap();
        // Because text is a sequence CRDT, both insertions survive instead of one
        // last-writer-wins clobbering the other. Order is deterministic.
        assert_eq!(merged.item_texts().unwrap(), vec!["Xhello!".to_string()]);
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
        let meta = doc.get_or_insert_map("meta");
        let mut txn = doc.transact_mut();
        meta.insert(&mut txn, "schema", "bad.workspace");
        meta.insert(&mut txn, "id", Workspace::new().id.to_string());
        meta.insert(&mut txn, "root", FolderId::new().to_string());
        meta.insert(&mut txn, "sync", "{}");
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
        metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
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
        metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
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

    #[test]
    fn concurrent_folder_additions_on_two_replicas_merge_without_loss() {
        // A shared base workspace that both replicas start from.
        let base = Workspace::new();

        // Each replica adds a different folder under the root and pushes its full
        // document state, exactly as a first-time/bootstrap sync does.
        let mut workspace_a = base.clone();
        let folder_x = add_root_folder(&mut workspace_a, "X");
        let a_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_a).updates;

        let mut workspace_b = base.clone();
        let folder_y = add_root_folder(&mut workspace_b, "Y");
        let b_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_b).updates;

        // The server holds the base and merges both replicas' deltas.
        let mut server = WorkspaceCrdtDocuments::try_new(&base).unwrap();
        let outcome_a = server.apply_remote_updates(&base, &stored_updates(base.id, a_updates));
        assert!(outcome_a.is_ok(), "{:?}", outcome_a.errors);
        let outcome_b =
            server.apply_remote_updates(&outcome_a.workspace, &stored_updates(base.id, b_updates));
        assert!(outcome_b.is_ok(), "{:?}", outcome_b.errors);

        // Both concurrently-added folders survive — neither clobbers the other the
        // way a single whole-document last-writer-wins blob would.
        let merged = outcome_b.workspace;
        assert!(merged.folders.contains_key(&folder_x), "folder X lost");
        assert!(merged.folders.contains_key(&folder_y), "folder Y lost");
        let root_children = &merged.folders[&merged.root].children;
        assert!(root_children.contains(&NodeRef::Folder(folder_x)));
        assert!(root_children.contains(&NodeRef::Folder(folder_y)));
    }

    #[test]
    fn concurrent_scheme_additions_under_root_merge_without_loss() {
        let base = Workspace::new();

        let mut workspace_a = base.clone();
        let scheme_a = add_root_scheme(&mut workspace_a, "A");
        let a_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_a).updates;

        let mut workspace_b = base.clone();
        let scheme_b = add_root_scheme(&mut workspace_b, "B");
        let b_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_b).updates;

        let mut server = WorkspaceCrdtDocuments::try_new(&base).unwrap();
        let outcome_a = server.apply_remote_updates(&base, &stored_updates(base.id, a_updates));
        assert!(outcome_a.is_ok(), "{:?}", outcome_a.errors);
        let outcome_b =
            server.apply_remote_updates(&outcome_a.workspace, &stored_updates(base.id, b_updates));
        assert!(outcome_b.is_ok(), "{:?}", outcome_b.errors);

        let merged = outcome_b.workspace;
        assert!(merged.schemes.contains_key(&scheme_a), "scheme A lost");
        assert!(merged.schemes.contains_key(&scheme_b), "scheme B lost");
        let root_children = &merged.folders[&merged.root].children;
        assert!(root_children.contains(&NodeRef::Scheme(scheme_a)));
        assert!(root_children.contains(&NodeRef::Scheme(scheme_b)));
    }

    fn add_root_folder(workspace: &mut Workspace, name: &str) -> FolderId {
        let folder = Folder {
            id: FolderId::new(),
            name: name.to_string(),
            parent: Some(workspace.root),
            children: Vec::new(),
            expanded: true,
        };
        let id = folder.id;
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(id));
        workspace.folders.insert(id, folder);
        workspace.ensure_sync_metadata();
        id
    }

    fn add_root_scheme(workspace: &mut Workspace, name: &str) -> SchemeId {
        let scheme = Scheme::new(name, 0);
        let id = scheme.id;
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(id));
        workspace.schemes.insert(id, scheme);
        workspace.ensure_sync_metadata();
        id
    }

    fn stored_updates(
        workspace_id: knotq_model::WorkspaceId,
        updates: Vec<CrdtDocumentUpdate>,
    ) -> Vec<StoredCrdtUpdate> {
        updates
            .into_iter()
            .enumerate()
            .map(|(index, update)| StoredCrdtUpdate {
                workspace_id,
                document: update.document,
                kind: update.kind,
                replica_id: knotq_model::ReplicaId::new(),
                sequence: (index + 1) as u64,
                received_at: chrono::Utc::now(),
                update_v1: update.update_v1,
            })
            .collect()
    }

    fn valid_single_item_scheme_doc() -> Doc {
        let doc = Doc::new();
        let metadata = doc.get_or_insert_map("scheme_file");
        let items_by_id = doc.get_or_insert_map("items_by_id");
        let scheme = Scheme::new("Plan", 0);
        let item = Item::new("First");
        let item_id = item.id.to_string();
        let mut txn = doc.transact_mut();
        metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
        metadata.insert(&mut txn, "id", scheme.id.to_string());
        items_by_id.insert(&mut txn, item_id, item_prelim(&item, "V").unwrap());
        drop(txn);
        doc
    }

    fn encode_full_update(doc: &Doc) -> Vec<u8> {
        doc.transact().encode_diff_v1(&StateVector::default())
    }
}
