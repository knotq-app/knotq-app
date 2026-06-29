//! The workspace-index CRDT (`YrsJsonDocument`): the folder/scheme tree, sync
//! metadata, daily queue and trash, each stored as id-keyed map entries so that
//! concurrent edits merge additively instead of as whole-document last-writer-wins.
use super::*;

/// The nine independent, id-keyed maps the workspace document is decomposed into.
/// `get` both creates them (first call on a fresh doc) and re-fetches them, in one
/// fixed order, so construction and reconciliation share a single source of truth.
struct WorkspaceMaps {
    meta: MapRef,
    nodes: MapRef,
    scheme_sync: MapRef,
    folder_sync: MapRef,
    daily_queue: MapRef,
    recently_deleted: MapRef,
    deleted_scheme_origins: MapRef,
    recently_deleted_folders: MapRef,
    deleted_folder_origins: MapRef,
}

impl WorkspaceMaps {
    fn get(doc: &Doc) -> Self {
        Self {
            meta: doc.get_or_insert_map("meta"),
            nodes: doc.get_or_insert_map("nodes"),
            scheme_sync: doc.get_or_insert_map("scheme_sync"),
            folder_sync: doc.get_or_insert_map("folder_sync"),
            daily_queue: doc.get_or_insert_map("daily_queue"),
            recently_deleted: doc.get_or_insert_map("recently_deleted"),
            deleted_scheme_origins: doc.get_or_insert_map("deleted_scheme_origins"),
            recently_deleted_folders: doc.get_or_insert_map("recently_deleted_folders"),
            deleted_folder_origins: doc.get_or_insert_map("deleted_folder_origins"),
        }
    }
}

/// Serialize a slice into `(key, json)` map entries — the shared shape of the
/// scheme/folder sync and deleted-origin maps in `replace_snapshot`.
fn json_map_entries<T, V: Serialize>(
    items: &[T],
    entry: impl Fn(&T) -> (String, &V),
) -> anyhow::Result<Vec<(String, String)>> {
    items
        .iter()
        .map(|item| {
            let (key, value) = entry(item);
            Ok((key, serde_json::to_string(value)?))
        })
        .collect()
}

pub(crate) struct YrsJsonDocument {
    pub(crate) id: DocumentId,
    pub(crate) kind: SyncDocumentKind,
    doc: Doc,
    encode_cache: EncodeCache,
}

impl YrsJsonDocument {
    pub(crate) fn new(id: DocumentId, kind: SyncDocumentKind) -> Self {
        let doc = Doc::new();
        // The workspace document is decomposed into independent, id-keyed maps so
        // that concurrent edits to distinct entities (e.g. two replicas each adding
        // a folder) merge additively instead of resolving as whole-document LWW.
        WorkspaceMaps::get(&doc);
        let encode_cache = EncodeCache::new(&doc);
        Self {
            id,
            kind,
            doc,
            encode_cache,
        }
    }

    pub(crate) fn new_with_client_id(id: DocumentId, kind: SyncDocumentKind, client_id: u64) -> Self {
        let doc = Doc::with_options(yrs_doc_options(id, client_id, OffsetKind::Bytes));
        WorkspaceMaps::get(&doc);
        let encode_cache = EncodeCache::new(&doc);
        Self {
            id,
            kind,
            doc,
            encode_cache,
        }
    }

    /// Build a workspace-index document whose clientID is either deterministic for
    /// `replica_id` (stable across reconstructions) or random when `None`.
    pub(crate) fn for_replica(id: DocumentId, kind: SyncDocumentKind, replica_id: Option<ReplicaId>) -> Self {
        match replica_id {
            Some(replica) => Self::new_with_client_id(id, kind, stable_client_id(replica, id)),
            None => Self::new(id, kind),
        }
    }

    /// Full document state as a v1 update, for durable persistence. Cached: the
    /// document is only re-serialized when it changed since the last call.
    pub(crate) fn encode_state_v1(&self) -> Vec<u8> {
        self.encode_cache
            .get(|| self.doc.transact().encode_diff_v1(&StateVector::default()))
    }

    /// Reconcile the persistent workspace document to `snapshot` and return the
    /// resulting update as an incremental diff from this document's own prior
    /// state. Encoding from the *persistent* doc (rather than a throwaway one) is
    /// essential: every op then carries this document's stable clientID and
    /// monotonically increasing clocks, so the same logical change keeps one
    /// identity across emits and replicas. A throwaway `Doc` would mint fresh
    /// clientIDs and clocks-from-zero for unchanged state, which Yjs then treats
    /// as competing concurrent writes whose last-writer-wins winner differs per
    /// replica — i.e. the workspace silently diverges (scheme names, archive
    /// state, ordering).
    ///
    /// When `force` is set (a sync document was added or removed) the full state
    /// is re-emitted instead of a diff, so a server that lost the document can
    /// rebuild it; the op ids are still the persistent doc's real ids, so the
    /// re-emit is idempotent on merge.
    pub(crate) fn sync_snapshot(
        &self,
        snapshot: &WorkspaceDocumentSnapshot,
        force: bool,
    ) -> anyhow::Result<Option<CrdtDocumentUpdate>> {
        let before = self.doc.transact().state_vector();
        let changed = self.replace_snapshot(snapshot)?;
        if !changed && !force {
            return Ok(None);
        }
        let base = if force {
            StateVector::default()
        } else {
            before
        };
        let update_v1 = self.doc.transact().encode_diff_v1(&base);
        if update_v1_is_empty(&update_v1) {
            return Ok(None);
        }
        Ok(Some(CrdtDocumentUpdate {
            document: self.id,
            kind: self.kind,
            update_v1,
        }))
    }

    pub(crate) fn replace_snapshot(&self, snapshot: &WorkspaceDocumentSnapshot) -> anyhow::Result<bool> {
        let WorkspaceMaps {
            meta,
            nodes,
            scheme_sync,
            folder_sync,
            daily_queue,
            recently_deleted,
            deleted_scheme_origins: deleted_origins,
            recently_deleted_folders,
            deleted_folder_origins,
        } = WorkspaceMaps::get(&self.doc);
        let mut txn = self.doc.transact_mut();

        // Reuse positions already stored so an unchanged tree re-serializes to
        // byte-identical entries, producing no update.
        let stored_node_positions = node_positions(&nodes, &txn);
        let stored_deleted_positions = string_map_entries(&recently_deleted, &txn)
            .into_iter()
            .collect::<HashMap<_, _>>();
        let stored_deleted_folder_positions = string_map_entries(&recently_deleted_folders, &txn)
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
                node_entry_json(
                    &id,
                    NODE_KIND_FOLDER,
                    &membership_parent,
                    &positions,
                    payload,
                )?,
            ));
        }
        for scheme in &snapshot.schemes {
            let id = scheme.id.to_string();
            ensure_position(&id, &mut positions);
            let payload = serde_json::to_string(scheme)?;
            node_entries.push((
                id.clone(),
                node_entry_json(
                    &id,
                    NODE_KIND_SCHEME,
                    &membership_parent,
                    &positions,
                    payload,
                )?,
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
            .map(|id| {
                (
                    id.clone(),
                    deleted_positions.get(id).cloned().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();

        let scheme_sync_entries =
            json_map_entries(&snapshot.scheme_sync, |e| (e.scheme.to_string(), &e.sync))?;
        let folder_sync_entries =
            json_map_entries(&snapshot.folder_sync, |e| (e.folder.to_string(), &e.sync))?;
        let mut daily_queue_entries = Vec::with_capacity(snapshot.daily_queue.len());
        for entry in &snapshot.daily_queue {
            daily_queue_entries.push((entry.date.to_string(), entry.scheme.to_string()));
        }
        let deleted_origin_entries = json_map_entries(&snapshot.deleted_scheme_origins, |e| {
            (e.scheme.to_string(), &e.origin)
        })?;

        // recently_deleted_folders is order-bearing too.
        let deleted_folder_ids = snapshot
            .recently_deleted_folders
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
        let mut deleted_folder_positions: HashMap<String, String> = HashMap::new();
        assign_fractional_positions(
            &deleted_folder_ids,
            &stored_deleted_folder_positions,
            &mut deleted_folder_positions,
        );
        let recently_deleted_folder_entries = deleted_folder_ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    deleted_folder_positions
                        .get(id)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        let deleted_folder_origin_entries =
            json_map_entries(&snapshot.deleted_folder_origins, |e| {
                (e.folder.to_string(), &e.origin)
            })?;

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
        changed |= sync_string_map(
            &recently_deleted_folders,
            &mut txn,
            &recently_deleted_folder_entries,
        );
        changed |= sync_string_map(
            &deleted_folder_origins,
            &mut txn,
            &deleted_folder_origin_entries,
        );
        Ok(changed)
    }

    pub(crate) fn apply_update_v1(&self, update: &[u8]) -> anyhow::Result<()> {
        self.doc
            .transact_mut()
            .apply_update(Update::decode_v1(update)?)?;
        Ok(())
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        match self.kind {
            SyncDocumentKind::PersonalWorkspace => validate_workspace_document(&self.doc),
            SyncDocumentKind::Scheme => validate_scheme_document(&self.doc),
            SyncDocumentKind::Folder => Err(anyhow!("folder CRDT documents are not supported")),
        }
    }

    pub(crate) fn snapshot(&self) -> anyhow::Result<WorkspaceDocumentSnapshot> {
        let WorkspaceMaps {
            meta,
            nodes,
            scheme_sync: scheme_sync_map,
            folder_sync: folder_sync_map,
            daily_queue: daily_queue_map,
            recently_deleted: recently_deleted_map,
            deleted_scheme_origins: deleted_origins_map,
            recently_deleted_folders: recently_deleted_folders_map,
            deleted_folder_origins: deleted_folder_origins_map,
        } = WorkspaceMaps::get(&self.doc);
        let txn = self.doc.transact();
        let raw_recently_deleted = string_map_entries(&recently_deleted_map, &txn);
        let raw_daily_queue = string_map_entries(&daily_queue_map, &txn);
        let raw_recently_deleted_folders = string_map_entries(&recently_deleted_folders_map, &txn);
        let deleted_scheme_ids = raw_recently_deleted
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<HashSet<_>>();
        let daily_queue_scheme_ids = raw_daily_queue
            .iter()
            .map(|(_, scheme)| scheme.clone())
            .collect::<HashSet<_>>();
        // Top-level archived folders (and, by walking the node parent links below,
        // their whole subtree). Archived folders are detached from the sidebar but
        // keep their internal structure so the archive can show them as folders.
        let archived_top_folder_ids = raw_recently_deleted_folders
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<HashSet<_>>();

        let read_meta = |key: &str| -> anyhow::Result<String> {
            meta.get_as::<_, Option<String>>(&txn, key)
                .with_context(|| format!("read workspace {key}"))?
                .ok_or_else(|| anyhow!("workspace {key} missing"))
        };
        let id = read_meta("id")?.parse().context("workspace id invalid")?;
        let root: FolderId = read_meta("root")?
            .parse()
            .context("workspace root invalid")?;
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

        // Walk the node parent links to find every folder inside an archived subtree,
        // starting from the archived top folders.
        let mut children_of: HashMap<String, Vec<String>> = HashMap::new();
        for (id_str, node) in &parsed {
            if !node.parent.is_empty() {
                children_of
                    .entry(node.parent.clone())
                    .or_default()
                    .push(id_str.clone());
            }
        }
        let mut archived_subtree_folders: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = archived_top_folder_ids.iter().cloned().collect();
        while let Some(current) = stack.pop() {
            if !folder_ids.contains(&current) || !archived_subtree_folders.insert(current.clone()) {
                continue;
            }
            for child in children_of.get(&current).into_iter().flatten() {
                if folder_ids.contains(child) {
                    stack.push(child.clone());
                }
            }
        }

        // Each node's effective parent is an existing folder, else the root —
        // orphans re-home under root rather than vanishing.
        let mut children_by_parent: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (id_str, node) in &parsed {
            if *id_str == root_key {
                continue;
            }
            // Archived top folders are detached from the sidebar: don't attach them to
            // any parent. Their subtree is still rebuilt under them below.
            if archived_top_folder_ids.contains(id_str) {
                continue;
            }
            if node.kind == NODE_KIND_SCHEME {
                let in_archived_subtree =
                    !node.parent.is_empty() && archived_subtree_folders.contains(&node.parent);
                // A deleted/daily scheme is kept out of the tree UNLESS it sits inside
                // an archived folder, where it must stay so the archive shows the
                // folder's contents.
                if !in_archived_subtree
                    && (deleted_scheme_ids.contains(id_str)
                        || daily_queue_scheme_ids.contains(id_str))
                {
                    continue;
                }
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

        let mut deleted = raw_recently_deleted
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

        let mut daily_queue = raw_daily_queue
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

        let mut deleted_folders = raw_recently_deleted_folders
            .into_iter()
            .map(|(id, position)| {
                let folder = id
                    .parse::<FolderId>()
                    .with_context(|| format!("recently deleted folder id invalid: {id}"))?;
                Ok::<_, anyhow::Error>((position, id, folder))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        deleted_folders.sort_by(|(lp, lid, _), (rp, rid, _)| lp.cmp(rp).then_with(|| lid.cmp(rid)));
        let recently_deleted_folders = deleted_folders
            .into_iter()
            .map(|(_, _, folder)| folder)
            .collect();

        let mut deleted_folder_origins = string_map_entries(&deleted_folder_origins_map, &txn)
            .into_iter()
            .map(|(folder, origin)| {
                Ok::<_, anyhow::Error>(DeletedFolderOriginEntry {
                    folder: folder
                        .parse()
                        .with_context(|| format!("deleted folder origin id invalid: {folder}"))?,
                    origin: serde_json::from_str(&origin)
                        .with_context(|| format!("deleted folder origin invalid: {folder}"))?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        deleted_folder_origins.sort_by_key(|entry| entry.folder.to_string());

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
            recently_deleted_folders,
            deleted_folder_origins,
            scheme_sync,
            folder_sync,
        })
    }
}

pub(crate) fn node_ref_id(node: &NodeRef) -> String {
    match node {
        NodeRef::Folder(id) => id.to_string(),
        NodeRef::Scheme(id) => id.to_string(),
    }
}

pub(crate) fn node_entry_json(
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
pub(crate) fn node_positions(map: &MapRef, txn: &impl ReadTxn) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (key, value) in string_map_entries(map, txn) {
        if let Ok(entry) = serde_json::from_str::<WorkspaceNodeEntry>(&value) {
            out.insert(key, entry.position);
        }
    }
    out
}

pub(crate) fn string_map_entries(map: &MapRef, txn: &impl ReadTxn) -> Vec<(String, String)> {
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
pub(crate) fn sync_string_map(map: &MapRef, txn: &mut TransactionMut, desired: &[(String, String)]) -> bool {
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
pub(crate) fn assign_fractional_positions(
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

pub(crate) fn workspace_document_snapshot(workspace: &Workspace) -> WorkspaceDocumentSnapshot {
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
            source: crdt_scheme_source(&scheme.source),
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

    let mut deleted_folder_origins = workspace
        .deleted_folder_origins
        .iter()
        .map(|(folder, origin)| DeletedFolderOriginEntry {
            folder: *folder,
            origin: *origin,
        })
        .collect::<Vec<_>>();
    deleted_folder_origins.sort_by_key(|entry| entry.folder.to_string());

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
        recently_deleted_folders: workspace.recently_deleted_folders.clone(),
        deleted_folder_origins,
        scheme_sync,
        folder_sync,
    }
}

pub(crate) fn crdt_scheme_source(source: &SchemeSource) -> SchemeSource {
    let mut source = source.clone();
    if let SchemeSource::ImportedCalendar(imported) = &mut source {
        imported.sync_token = None;
    }
    source
}

pub(crate) fn preserve_local_calendar_sync_token(
    current: &Workspace,
    scheme_id: SchemeId,
    mut remote_source: SchemeSource,
) -> SchemeSource {
    let SchemeSource::ImportedCalendar(remote) = &mut remote_source else {
        return remote_source;
    };
    if remote.sync_token.is_some() {
        return remote_source;
    }
    let Some(SchemeSource::ImportedCalendar(local)) =
        current.schemes.get(&scheme_id).map(|scheme| &scheme.source)
    else {
        return remote_source;
    };
    if local.provider == remote.provider
        && local.account_id == remote.account_id
        && local.calendar_id == remote.calendar_id
    {
        remote.sync_token = local.sync_token.clone();
    }
    remote_source
}

pub(crate) fn scheme_meta(workspace: &Workspace, id: SchemeId) -> anyhow::Result<&SyncDocumentMeta> {
    workspace
        .scheme_sync
        .get(&id)
        .ok_or_else(|| anyhow!("workspace missing scheme sync metadata for {id}"))
}

pub(crate) fn scheme_documents_by_id(workspace: &Workspace) -> HashMap<knotq_model::DocumentId, SchemeId> {
    workspace
        .scheme_sync
        .iter()
        .filter(|(_, meta)| meta.kind == SyncDocumentKind::Scheme)
        .map(|(scheme, meta)| (meta.id, *scheme))
        .collect()
}
