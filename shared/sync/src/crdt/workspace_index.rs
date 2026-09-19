//! The workspace-index CRDT (`YrsJsonDocument`): the folder/scheme tree, sync
//! metadata, daily queue and trash, each stored as id-keyed map entries so that
//! concurrent edits merge additively instead of as whole-document last-writer-wins.
use super::*;

/// The ten independent, id-keyed maps the workspace document is decomposed into.
/// `get` both creates them (first call on a fresh doc) and re-fetches them, in one
/// fixed order, so construction and reconciliation share a single source of truth.
struct WorkspaceMaps {
    meta: MapRef,
    nodes: MapRef,
    /// One key per mutable field of a node (`<id>\u{1f}<field>`), including its
    /// membership parent and position, alongside the whole-node value in `nodes`.
    ///
    /// `nodes` keeps a node's every field — name, colour, gsync, source, and its
    /// membership parent and position — inside ONE map value, so two devices
    /// editing DIFFERENT fields of the same scheme write the same key and Yjs
    /// resolves the whole node by client id: the loser's edit is silently
    /// discarded. `write_item_fields` fixed exactly this for item metadata
    /// ("writing every field on any edit let a device that changed one attribute
    /// silently restore its stale copy of every other attribute"); the index
    /// never got the same treatment.
    ///
    /// Splitting the fields here gives each one its own last-writer-wins key, so
    /// concurrent edits to different fields merge. `nodes` is still written
    /// unchanged and is still the fallback on read, which is what keeps this
    /// backward compatible: an older build reads the maps it knows and never
    /// sees this one, and neither the client's `validate_workspace_document` nor
    /// the Worker's `crdt_validation.ts` enumerates top-level maps.
    node_fields: MapRef,
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
            node_fields: doc.get_or_insert_map("node_fields"),
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

/// Separator between a node id and a field name in `node_fields`. A unit
/// separator cannot occur in a UUID or a field name, so the key splits back
/// unambiguously.
const NODE_FIELD_SEPARATOR: char = '\u{1f}';

/// Stamped into every `nodes` entry this build writes, to mark that the matching
/// `node_fields` keys were written alongside it.
///
/// A build that predates `node_fields` cannot write those keys — it does not know
/// the map exists — and `sync_string_map` only prunes keys inside the map it is
/// handed, so it cannot clear them either. Without this stamp such a build's
/// rename or recolour is silently reverted on every build that prefers the
/// per-field keys, which then re-asserts the stale value on its next write.
///
/// Only the stamp's PRESENCE is consulted, never its value: presence means "the
/// field keys belong to this payload", absence means "this payload came from a
/// build that could not maintain them, so trust it instead". A constant keeps
/// re-serialization byte-identical, so an unchanged workspace still emits no
/// update (`store_tests`: "an unchanged workspace must not queue new CRDT edits").
///
/// Removable once every client is past the `node_fields` change — see
/// `CLIENT_SYNC_PROTOCOL_VERSION` and the backend's
/// `MIN_SUPPORTED_CLIENT_SYNC_PROTOCOL_VERSION`.
const NODE_FIELD_SCHEMA: u32 = 1;

/// The `node_fields` key for one field of one node.
fn node_field_key(id: &str, field: &str) -> String {
    format!("{id}{NODE_FIELD_SEPARATOR}{field}")
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
    /// Collects the bytes of the transactions `replace_snapshot` commits, so an
    /// ordinary edit emits the change itself rather than a diff recomputed by
    /// walking the whole document. See `update_capture`.
    capture: UpdateCapture,
}

impl YrsJsonDocument {
    pub(crate) fn new(id: DocumentId, kind: SyncDocumentKind) -> Self {
        // Built exactly like `new_with_client_id` (same guid, same `OffsetKind`),
        // differing only in taking a fresh random clientID. It previously used a
        // bare `Doc::new()`, which left the document without its guid and — the
        // part that matters — took yrs's *unpartitioned* default clientID rather
        // than one in the document namespace, so a fresh workspace document could
        // alias an item-skeleton seed clientID. See `random_document_client_id`
        // and `stable_item_seed_client_id`; the scheme document has always done
        // this correctly.
        let doc = Doc::with_options(yrs_doc_options(
            id,
            random_document_client_id(),
            OffsetKind::Bytes,
        ));
        // The workspace document is decomposed into independent, id-keyed maps so
        // that concurrent edits to distinct entities (e.g. two replicas each adding
        // a folder) merge additively instead of resolving as whole-document LWW.
        WorkspaceMaps::get(&doc);
        let encode_cache = EncodeCache::new(&doc);
        let capture = UpdateCapture::install(&doc);
        Self {
            id,
            kind,
            doc,
            encode_cache,
            capture,
        }
    }

    pub(crate) fn new_with_client_id(
        id: DocumentId,
        kind: SyncDocumentKind,
        client_id: u64,
    ) -> Self {
        let doc = Doc::with_options(yrs_doc_options(id, client_id, OffsetKind::Bytes));
        WorkspaceMaps::get(&doc);
        let encode_cache = EncodeCache::new(&doc);
        let capture = UpdateCapture::install(&doc);
        Self {
            id,
            kind,
            doc,
            encode_cache,
            capture,
        }
    }

    /// Build a workspace-index document whose clientID is either deterministic for
    /// `replica_id` (stable across reconstructions) or random when `None`.
    pub(crate) fn for_replica(
        id: DocumentId,
        kind: SyncDocumentKind,
        replica_id: Option<ReplicaId>,
    ) -> Self {
        match replica_id {
            Some(replica) => Self::new_with_client_id(id, kind, stable_client_id(replica, id)),
            None => Self::new(id, kind),
        }
    }

    /// This document's authoring clientID.
    pub(crate) fn client_id(&self) -> u64 {
        self.doc.client_id().get()
    }

    /// Full document state as a v1 update, for durable persistence. Cached: the
    /// document is only re-serialized when it changed since the last call.
    pub(crate) fn encode_state_v1(&self) -> Vec<u8> {
        self.encode_cache
            .get(|| self.doc.transact().encode_diff_v1(&StateVector::default()))
    }

    /// The same state as [`Self::encode_state_v1`], shared rather than copied.
    pub(crate) fn encode_state_shared_v1(&self) -> std::sync::Arc<[u8]> {
        self.encode_cache
            .get_shared(|| self.doc.transact().encode_diff_v1(&StateVector::default()))
    }

    pub(crate) fn state_vector_v1(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// A handle that produces the same state from another thread.
    pub(crate) fn state_handle(&self) -> DocumentStateHandle {
        self.encode_cache.handle(&self.doc)
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
        // `force` deliberately re-emits the WHOLE document, so it never uses
        // what the write produced; an ordinary edit takes the delta from the
        // writes themselves rather than diffing the document afterwards (see
        // `update_capture`).
        let delta = if force {
            Delta::Diff(StateVector::default())
        } else {
            match self.capture.arm() {
                Some(guard) => Delta::Captured(guard),
                None => Delta::Diff(self.doc.transact().state_vector()),
            }
        };
        let changed = self.replace_snapshot(snapshot)?;
        if !changed && !force {
            return Ok(None);
        }
        let update_v1 = match delta {
            Delta::Captured(guard) => guard.finish()?,
            Delta::Diff(base) => self.doc.transact().encode_diff_v1(&base),
        };
        if update_v1_is_empty(&update_v1) {
            return Ok(None);
        }
        Ok(Some(CrdtDocumentUpdate {
            document: self.id,
            kind: self.kind,
            update_v1,
            // Touched-item tracking is a scheme-content concept; the workspace
            // index is never squashed, so its updates carry none.
            touched_items: Vec::new(),
        }))
    }

    /// Write `content` into this unseeded document as one deterministic update:
    /// built in a scratch document whose clientID is a hash of the document id
    /// and the exact content (`stable_workspace_population_client_id`), then
    /// applied here. Mirrors `YrsSchemeDocument::populate` — see that method's
    /// doc comment for why: every replica populating this document from the
    /// SAME content produces byte-identical operations, which Yjs integrates
    /// once instead of treating every node as a genuinely concurrent write
    /// decided by clientID alone.
    pub(crate) fn populate(&self, content: &WorkspaceDocumentSnapshot) -> anyhow::Result<()> {
        let key = serde_json::to_vec(content)?;
        let client_id = super::encoding::stable_workspace_population_client_id(self.id, &key);
        let scratch = Self::new_with_client_id(self.id, self.kind, client_id);
        scratch.replace_snapshot(content)?;
        let population = scratch.encode_state_v1();
        self.doc
            .transact_mut()
            .apply_update(Update::decode_v1(&population)?)?;
        Ok(())
    }

    /// Re-key an already-populated document onto a canonical identity, keeping
    /// any edit made on top of its (pre-canonicalization) population.
    ///
    /// A device populates its workspace-index document deterministically from
    /// whatever workspace content it holds at the time (see [`Self::populate`]).
    /// Before this device has adopted the account's canonical identity, that
    /// content still carries this device's own per-install-random `sync.id` —
    /// baked into the population's content hash — so it can never match
    /// another device's population of the SAME logical starter content under
    /// the account's real identity. A plain re-key
    /// ([`super::WorkspaceCrdtDocuments::reidentify_workspace_document`]) only
    /// rebinds the document's external id; the wrong-hashed population inside
    /// it is unchanged, so it still won't deduplicate.
    ///
    /// Fix: build a fresh document, populate it from `canonical_base` (the
    /// same content this document was populated from, canonicalized), then
    /// write `edited_canonical` (this document's CURRENT content —
    /// canonicalized the same way, edits included) into it as an ordinary
    /// `replace_snapshot` — exactly how the original edit was written the
    /// first time. That makes the edit a fresh, direct write on top of the
    /// canonical population, causally following it — not a replayed byte
    /// diff whose origin pointers reference structs the canonical population
    /// never had, which the previous version of this method got wrong (it
    /// left the edit competing with another replica's identical population
    /// write as if genuinely concurrent, decided by clientID and liable to
    /// lose). The population portion is unaffected either way — it collapses
    /// into a no-op against a matching population, by the same mechanism
    /// [`Self::populate`] relies on.
    pub(crate) fn repopulate_canonically(
        &self,
        canonical_base: &WorkspaceDocumentSnapshot,
        edited_canonical: &WorkspaceDocumentSnapshot,
        new_id: DocumentId,
    ) -> anyhow::Result<Self> {
        let fresh = Self::new(new_id, self.kind);
        fresh.populate(canonical_base)?;
        fresh.replace_snapshot(edited_canonical)?;
        Ok(fresh)
    }

    pub(crate) fn replace_snapshot(
        &self,
        snapshot: &WorkspaceDocumentSnapshot,
    ) -> anyhow::Result<bool> {
        let WorkspaceMaps {
            meta,
            nodes,
            node_fields,
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
        let permanently_deleted_scheme_ids = snapshot
            .deleted_scheme_origins
            .iter()
            .map(|entry| entry.scheme)
            .filter(|id| {
                snapshot
                    .deleted_scheme_origins
                    .iter()
                    .find(|entry| entry.scheme == *id)
                    .is_some_and(|entry| {
                        entry.origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION
                    })
            })
            .collect::<HashSet<_>>();
        let permanently_deleted_folder_ids = snapshot
            .deleted_folder_origins
            .iter()
            .map(|entry| entry.folder)
            .filter(|id| {
                snapshot
                    .deleted_folder_origins
                    .iter()
                    .find(|entry| entry.folder == *id)
                    .is_some_and(|entry| {
                        entry.origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION
                    })
            })
            .collect::<HashSet<_>>();

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
        // Sorted by parent: `assign_fractional_positions` mints position keys
        // into the shared `positions` map, so iterating the parents in HashMap
        // order let the same node come out with a different key run to run —
        // and device to device, which is divergence, not just irreproducibility.
        let mut parents: Vec<&String> = children_by_parent.keys().collect();
        parents.sort();
        for parent in parents {
            assign_fractional_positions(
                &children_by_parent[parent],
                &stored_node_positions,
                &mut positions,
            );
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
            if permanently_deleted_folder_ids.contains(&folder.id) {
                continue;
            }
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
            if permanently_deleted_scheme_ids.contains(&scheme.id) {
                continue;
            }
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
            .filter(|id| {
                id.parse::<SchemeId>()
                    .map(|id| !permanently_deleted_scheme_ids.contains(&id))
                    .unwrap_or(true)
            })
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
            json_map_entries(&snapshot.scheme_sync, |e| (e.scheme.to_string(), &e.sync))?
                .into_iter()
                .filter(|(id, _)| {
                    id.parse::<SchemeId>()
                        .map(|id| !permanently_deleted_scheme_ids.contains(&id))
                        .unwrap_or(true)
                })
                .collect::<Vec<_>>();
        let folder_sync_entries =
            json_map_entries(&snapshot.folder_sync, |e| (e.folder.to_string(), &e.sync))?
                .into_iter()
                .filter(|(id, _)| {
                    id.parse::<FolderId>()
                        .map(|id| !permanently_deleted_folder_ids.contains(&id))
                        .unwrap_or(true)
                })
                .collect::<Vec<_>>();
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
            .filter(|id| {
                id.parse::<FolderId>()
                    .map(|id| !permanently_deleted_folder_ids.contains(&id))
                    .unwrap_or(true)
            })
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
        // A Daily Queue page that is still bound but not loaded — outside the
        // date window this device loaded — is absent from `snapshot.schemes`.
        // Keep its stored entry exactly as it is. Rewriting the nodes from the
        // loaded schemes alone deleted it: the day's binding and lines survived
        // but no device could materialize the day any more.
        let mut listed: HashSet<String> = snapshot
            .schemes
            .iter()
            .map(|scheme| scheme.id.to_string())
            .collect();
        let stored_nodes: HashMap<String, String> =
            string_map_entries(&nodes, &txn).into_iter().collect();
        // The plain workspace can omit any scheme whose file was not loaded at
        // this boundary, not only Daily Queue pages. `scheme_sync` is the
        // durable ownership/index set; after `ensure_sync_metadata`, a binding
        // there is live (or an intentionally retained daily) and must keep its
        // node entry even when the materialized scheme is absent.
        let retained_scheme_ids: Vec<String> = snapshot
            .scheme_sync
            .iter()
            .filter(|entry| entry.sync.kind == SyncDocumentKind::Scheme)
            .filter(|entry| !permanently_deleted_scheme_ids.contains(&entry.scheme))
            .map(|entry| entry.scheme.to_string())
            .collect();
        for id in retained_scheme_ids {
            if !listed.insert(id.clone()) {
                continue;
            }
            if let Some(stored) = stored_nodes.get(&id) {
                node_entries.push((id, stored.clone()));
            }
        }
        // One key per mutable field, so two devices editing DIFFERENT fields of
        // the same node no longer collide on a single value. `sync_string_map`
        // writes only keys whose value actually changed, which is what makes
        // this work: a device that changes one field writes that one key and
        // leaves every other field's key — and so a concurrent edit to it —
        // untouched. `nodes` above is still written whole and unchanged, so an
        // older build keeps reading exactly what it reads today.
        let stored_node_fields: HashMap<String, String> =
            string_map_entries(&node_fields, &txn).into_iter().collect();
        let mut node_field_entries: Vec<(String, String)> = Vec::new();
        let mut rebuilt_nodes: HashSet<String> = HashSet::new();
        for folder in &snapshot.folders {
            if permanently_deleted_folder_ids.contains(&folder.id) {
                continue;
            }
            let id = folder.id.to_string();
            node_field_entries.push((node_field_key(&id, "name"), folder.name.clone()));
            node_field_entries.push((
                node_field_key(&id, "membership_parent"),
                membership_parent.get(&id).cloned().unwrap_or_default(),
            ));
            node_field_entries.push((
                node_field_key(&id, "position"),
                positions.get(&id).cloned().unwrap_or_default(),
            ));
            node_field_entries.push((node_field_key(&id, "expanded"), folder.expanded.to_string()));
            node_field_entries.push((
                node_field_key(&id, "parent"),
                folder
                    .parent
                    .map(|parent| parent.to_string())
                    .unwrap_or_default(),
            ));
            rebuilt_nodes.insert(id);
        }
        for scheme in &snapshot.schemes {
            if permanently_deleted_scheme_ids.contains(&scheme.id) {
                continue;
            }
            let id = scheme.id.to_string();
            node_field_entries.push((node_field_key(&id, "name"), scheme.name.clone()));
            node_field_entries.push((
                node_field_key(&id, "membership_parent"),
                membership_parent.get(&id).cloned().unwrap_or_default(),
            ));
            node_field_entries.push((
                node_field_key(&id, "position"),
                positions.get(&id).cloned().unwrap_or_default(),
            ));
            node_field_entries.push((
                node_field_key(&id, "color_index"),
                scheme.color_index.to_string(),
            ));
            node_field_entries.push((node_field_key(&id, "gsync"), scheme.gsync.to_string()));
            node_field_entries.push((
                node_field_key(&id, "source"),
                serde_json::to_string(&scheme.source)?,
            ));
            rebuilt_nodes.insert(id);
        }
        // A Daily page outside the loaded window keeps its stored `nodes` entry
        // above rather than being rebuilt from a workspace that does not hold it.
        // Its field keys must be kept the same way: `sync_string_map` removes
        // every key absent from `desired`, so omitting them here would delete the
        // page's metadata for the whole account (the "server lost Daily page"
        // failure, in a new map).
        for (id, _) in &node_entries {
            if rebuilt_nodes.contains(id) {
                continue;
            }
            for (key, value) in &stored_node_fields {
                if key.split(NODE_FIELD_SEPARATOR).next() == Some(id.as_str()) {
                    node_field_entries.push((key.clone(), value.clone()));
                }
            }
        }
        changed |= sync_string_map(&nodes, &mut txn, &node_entries);
        changed |= sync_string_map(&node_fields, &mut txn, &node_field_entries);
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

    /// Applies a remote update and reports whether it changed this document.
    /// An echo of state this replica already holds (e.g. the server broadcasting
    /// our own push back) merges as a no-op; comparing the (state vector,
    /// delete set) snapshot before/after detects that — including delete-only
    /// updates, which advance no clock.
    pub(crate) fn apply_update_v1(&self, update: &[u8]) -> anyhow::Result<bool> {
        let mut txn = self.doc.transact_mut();
        let before = txn.snapshot();
        txn.apply_update(Update::decode_v1(update)?)?;
        Ok(txn.snapshot() != before)
    }

    /// Whether this document has ever been written or restored, as opposed to
    /// being the bare set of empty root maps a constructor leaves behind.
    ///
    /// `meta.id` is the marker: every path that seeds a workspace document
    /// writes it, and nothing removes it.
    pub(crate) fn is_seeded(&self) -> bool {
        let meta = self.doc.get_or_insert_map("meta");
        let txn = self.doc.transact();
        meta.get_as::<_, Option<String>>(&txn, "id")
            .ok()
            .flatten()
            .is_some()
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
            node_fields,
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
        let raw_deleted_scheme_origins = string_map_entries(&deleted_origins_map, &txn);
        let raw_deleted_folder_origins = string_map_entries(&deleted_folder_origins_map, &txn);
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
        // An origin without a matching archive entry is the durable marker for
        // a permanent delete. Ignore the old node even if a stale replica
        // reintroduces it into the additive CRDT map.
        let permanently_deleted_scheme_ids = raw_deleted_scheme_origins
            .iter()
            .filter_map(|(id, raw)| {
                serde_json::from_str::<DeletedSchemeOrigin>(raw)
                    .ok()
                    .filter(|origin| origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION)
                    .map(|_| id.clone())
            })
            .collect::<HashSet<_>>();
        let permanently_deleted_folder_ids = raw_deleted_folder_origins
            .iter()
            .filter_map(|(id, raw)| {
                serde_json::from_str::<DeletedFolderOrigin>(raw)
                    .ok()
                    .filter(|origin| origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION)
                    .map(|_| id.clone())
            })
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
            /// See [`NODE_FIELD_SCHEMA`]: `None` means an older build wrote this
            /// entry, so its payload wins over any `node_fields` key.
            field_schema: Option<u32>,
        }
        let mut parsed: HashMap<String, ParsedNode> = HashMap::new();
        let mut folder_ids: HashSet<String> = HashSet::new();
        for (key, value) in string_map_entries(&nodes, &txn) {
            if permanently_deleted_scheme_ids.contains(&key)
                || permanently_deleted_folder_ids.contains(&key)
            {
                continue;
            }
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
                    field_schema: entry.field_schema,
                },
            );
        }

        // Prefer the per-field keys over the whole-node payload. A field written
        // to `node_fields` merged independently of every other field, while the
        // payload resolved the WHOLE node last-writer-wins and can carry another
        // device's stale copy of a field this one changed. A node with no field
        // keys — one written by an older build — keeps its payload verbatim.
        // Applying this to the parsed payload rather than at each use means the
        // legacy-root detection, the parent remapping and the scheme parse below
        // all read the merged values.
        let stored_node_fields = string_map_entries(&node_fields, &txn);
        if !stored_node_fields.is_empty() {
            let mut by_node: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
            for (key, value) in &stored_node_fields {
                if let Some((id, field)) = key.split_once(NODE_FIELD_SEPARATOR) {
                    by_node.entry(id).or_default().push((field, value.as_str()));
                }
            }
            for (id, fields) in by_node {
                let Some(node) = parsed.get_mut(id) else {
                    continue;
                };
                // An unstamped entry was written by a build that predates
                // `node_fields` (see `NODE_FIELD_SCHEMA`). It could not have
                // written these keys, so they are stale and its payload — which
                // carries that build's actual edit — is the authority. Applying
                // them here is what silently reverted an older device's rename or
                // recolour on every newer device.
                if node.field_schema.is_none() {
                    continue;
                }
                let Ok(mut payload) = serde_json::from_str::<serde_json::Value>(&node.payload)
                else {
                    continue;
                };
                let Some(object) = payload.as_object_mut() else {
                    continue;
                };
                for (field, raw) in fields {
                    let value = match field {
                        "membership_parent" => {
                            node.parent = raw.to_string();
                            continue;
                        }
                        "position" => {
                            node.position = raw.to_string();
                            continue;
                        }
                        "name" => serde_json::Value::String(raw.to_string()),
                        "expanded" | "gsync" => match raw {
                            "true" => serde_json::Value::Bool(true),
                            "false" => serde_json::Value::Bool(false),
                            _ => continue,
                        },
                        "color_index" => match raw.parse::<u64>() {
                            Ok(number) => serde_json::Value::Number(number.into()),
                            Err(_) => continue,
                        },
                        "parent" if raw.is_empty() => serde_json::Value::Null,
                        "parent" => serde_json::Value::String(raw.to_string()),
                        "source" => match serde_json::from_str(raw) {
                            Ok(source) => source,
                            Err(_) => continue,
                        },
                        _ => continue,
                    };
                    object.insert(field.to_string(), value);
                }
                node.payload = serde_json::to_string(&payload)?;
            }
        }

        let root_key = root.to_string();

        // A first-sync merge can leave the pre-sign-in root as a second folder
        // node. The node membership already tells us that it belongs under the
        // canonical root, while its older payload still says `parent: null`.
        // Treat that legacy root as an alias here: its children survive under the
        // canonical root, but the alias itself must not materialize as a sidebar
        // folder or change parent on the next relaunch.
        let legacy_root_ids: HashSet<String> = parsed
            .iter()
            .filter_map(|(id, node)| {
                if id == &root_key || node.kind != NODE_KIND_FOLDER {
                    return None;
                }
                let payload = serde_json::from_str::<FolderPayload>(&node.payload).ok()?;
                (payload.name == "root"
                    && (payload.parent.is_none() || payload.parent == Some(root)))
                .then_some(id.clone())
            })
            .collect();

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
            if legacy_root_ids.contains(id_str) {
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
            let parent = if legacy_root_ids.contains(&node.parent) {
                root_key.clone()
            } else if !node.parent.is_empty() && folder_ids.contains(&node.parent) {
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
                if legacy_root_ids.contains(id_str) {
                    continue;
                }
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
                let id = id_str
                    .parse::<FolderId>()
                    .with_context(|| format!("folder id invalid: {id_str}"))?;
                let parent = if id == root {
                    None
                } else if archived_top_folder_ids.contains(id_str) {
                    payload.parent
                } else {
                    // `node.parent` is the independently merged membership
                    // relation. The payload's parent is a redundant copy that
                    // can come from a losing whole-node write; using it here
                    // can rehome a folder even though its membership survived.
                    let parent = node
                        .parent
                        .parse::<FolderId>()
                        .ok()
                        .filter(|parent| folder_ids.contains(&parent.to_string()))
                        .unwrap_or(root);
                    Some(if legacy_root_ids.contains(&parent.to_string()) {
                        root
                    } else {
                        parent
                    })
                };
                folders.push(Folder {
                    id,
                    name: payload.name,
                    parent,
                    children,
                    expanded: payload.expanded,
                });
            } else {
                let entry: SchemeWorkspaceEntry = serde_json::from_str(&node.payload)
                    .with_context(|| format!("scheme payload invalid: {id_str}"))?;
                schemes.push(entry);
            }
        }
        // The index's root can have no folder node of its own — two histories
        // merged on an account switch with one root id winning `meta.root` while
        // the other's node is gone. Nodes whose parent is missing were re-homed
        // under the root above; without a folder to hold them they would belong
        // to no folder at all, and normalization drops every such scheme (the
        // identity repair then wrote that as the account's index, deleting the
        // schemes for every device). Give the root its folder.
        if !folder_ids.contains(&root_key) {
            let children = children_by_parent
                .get(&root_key)
                .map(|kids| {
                    kids.iter()
                        .map(|(_, child_id)| node_ref_for(child_id))
                        .collect::<anyhow::Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            folders.push(Folder {
                id: root,
                name: "root".to_string(),
                parent: None,
                children,
                expanded: true,
            });
        }
        folders.sort_by_key(|folder| folder.id.to_string());
        schemes.sort_by_key(|scheme| scheme.id.to_string());

        let mut deleted = raw_recently_deleted
            .into_iter()
            .filter(|(id, _)| !permanently_deleted_scheme_ids.contains(id))
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
            .filter(|(scheme, _)| !permanently_deleted_scheme_ids.contains(scheme))
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
            .filter(|(folder, _)| !permanently_deleted_folder_ids.contains(folder))
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
            .filter(|(id, _)| !permanently_deleted_folder_ids.contains(id))
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
        // This build writes the per-field keys alongside the entry, so the
        // entry is stamped. See `NODE_FIELD_SCHEMA`.
        field_schema: Some(NODE_FIELD_SCHEMA),
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
pub(crate) fn sync_string_map(
    map: &MapRef,
    txn: &mut TransactionMut,
    desired: &[(String, String)],
) -> bool {
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
        // An origin without a live/archive scheme is a permanent-delete
        // tombstone. Do not keep its content-document binding alive in the
        // workspace index, or the stale node can be retained by the lazy-scheme
        // preservation path below.
        .filter(|(scheme, _)| {
            !workspace
                .deleted_scheme_origins
                .get(scheme)
                .is_some_and(|origin| origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION)
                && (workspace.schemes.contains_key(scheme)
                    || workspace.recently_deleted.contains(scheme)
                    || !workspace.deleted_scheme_origins.contains_key(scheme))
        })
        .map(|(scheme, sync)| SchemeSyncEntry {
            scheme: *scheme,
            sync: sync.clone(),
        })
        .collect::<Vec<_>>();
    scheme_sync.sort_by_key(|entry| entry.scheme.to_string());

    let mut folder_sync = workspace
        .folder_sync
        .iter()
        .filter(|(folder, _)| {
            !workspace
                .deleted_folder_origins
                .get(folder)
                .is_some_and(|origin| origin.position == PERMANENT_DELETE_TOMBSTONE_POSITION)
                && (workspace.folders.contains_key(folder)
                    || workspace.recently_deleted_folders.contains(folder)
                    || !workspace.deleted_folder_origins.contains_key(folder))
        })
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

pub(crate) fn scheme_meta(
    workspace: &Workspace,
    id: SchemeId,
) -> anyhow::Result<&SyncDocumentMeta> {
    workspace
        .scheme_sync
        .get(&id)
        .ok_or_else(|| anyhow!("workspace missing scheme sync metadata for {id}"))
}

pub(crate) fn scheme_documents_by_id(
    workspace: &Workspace,
) -> HashMap<knotq_model::DocumentId, SchemeId> {
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

    /// Build a one-scheme workspace and write its index the way this build does.
    fn indexed_workspace(colour: u8) -> (YrsJsonDocument, SchemeId) {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Plans", colour);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));
        workspace.ensure_sync_metadata();
        let doc = YrsJsonDocument::new(workspace.sync.id, SyncDocumentKind::PersonalWorkspace);
        doc.replace_snapshot(&workspace_document_snapshot(&workspace))
            .expect("first index write");
        (doc, scheme_id)
    }

    fn colour_of(doc: &YrsJsonDocument, scheme: SchemeId) -> u8 {
        doc.snapshot()
            .expect("read the index back")
            .schemes
            .iter()
            .find(|entry| entry.id == scheme)
            .expect("the scheme survives")
            .color_index
    }

    #[test]
    fn independently_edited_index_fields_merge_after_shared_population() {
        let (workspace, scheme_id) = one_scheme_workspace(0);
        let base = workspace_document_snapshot(&workspace);
        let mut renamed = workspace.clone();
        renamed.schemes.get_mut(&scheme_id).unwrap().name = "Renamed".to_string();
        let mut recoloured = workspace;
        recoloured.schemes.get_mut(&scheme_id).unwrap().color_index = 7;

        let left = YrsJsonDocument::new(base.sync.id, SyncDocumentKind::PersonalWorkspace);
        let right = YrsJsonDocument::new(base.sync.id, SyncDocumentKind::PersonalWorkspace);
        left.populate(&base).expect("left population");
        right.populate(&base).expect("right population");
        left.replace_snapshot(&workspace_document_snapshot(&renamed))
            .expect("left rename");
        right
            .replace_snapshot(&workspace_document_snapshot(&recoloured))
            .expect("right recolour");

        left.apply_update_v1(&right.encode_state_v1())
            .expect("merge right into left");
        right
            .apply_update_v1(&left.encode_state_v1())
            .expect("merge left into right");

        let left = left.snapshot().expect("left snapshot");
        let right = right.snapshot().expect("right snapshot");
        let left_scheme = left
            .schemes
            .iter()
            .find(|entry| entry.id == scheme_id)
            .unwrap();
        let right_scheme = right
            .schemes
            .iter()
            .find(|entry| entry.id == scheme_id)
            .unwrap();
        assert_eq!(left_scheme.name, "Renamed");
        assert_eq!(right_scheme.name, "Renamed");
        assert_eq!(left_scheme.color_index, 7);
        assert_eq!(right_scheme.color_index, 7);
    }

    fn indexed_workspace_with_folder() -> (YrsJsonDocument, SchemeId, FolderId) {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Plans", 3);
        let scheme_id = scheme.id;
        let folder_id = FolderId::new();
        workspace.schemes.insert(scheme_id, scheme);
        workspace.folders.insert(
            folder_id,
            Folder {
                id: folder_id,
                name: "Destination".to_string(),
                parent: Some(workspace.root),
                children: vec![NodeRef::Scheme(scheme_id)],
                expanded: true,
            },
        );
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(folder_id));
        workspace.ensure_sync_metadata();
        let doc = YrsJsonDocument::new(workspace.sync.id, SyncDocumentKind::PersonalWorkspace);
        doc.replace_snapshot(&workspace_document_snapshot(&workspace))
            .expect("first index write");
        (doc, scheme_id, folder_id)
    }

    #[test]
    fn archived_folder_round_trips_through_incremental_index_update() {
        let mut workspace = Workspace::new();
        let folder_id = FolderId::new();
        workspace.folders.insert(
            folder_id,
            Folder {
                id: folder_id,
                name: "Archive me".to_string(),
                parent: Some(workspace.root),
                children: Vec::new(),
                expanded: true,
            },
        );
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(folder_id));
        workspace.ensure_sync_metadata();
        let doc = YrsJsonDocument::new(workspace.sync.id, SyncDocumentKind::PersonalWorkspace);
        doc.replace_snapshot(&workspace_document_snapshot(&workspace))
            .expect("initial index write");
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .clear();
        workspace.mark_folder_deleted_from(folder_id, workspace.root, 0);
        doc.replace_snapshot(&workspace_document_snapshot(&workspace))
            .expect("archive index write");
        assert!(doc
            .snapshot()
            .expect("materialize archive")
            .recently_deleted_folders
            .contains(&folder_id));
    }

    fn one_scheme_workspace(colour: u8) -> (Workspace, SchemeId) {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Plans", colour);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));
        workspace.ensure_sync_metadata();
        (workspace, scheme_id)
    }

    /// FIRST-SYNC IDENTITY RACE, part 1: `populate` must be a pure function of
    /// content, not of which device (or how many prior writes) called it.
    /// Without this, two devices independently populating the workspace-index
    /// document from the SAME account content author it under two different,
    /// effectively-random clientIDs, and Yjs resolves every entry as a
    /// genuinely concurrent write decided by clientID alone -- silently
    /// discarding one side's, even when nothing was actually edited.
    #[test]
    fn two_independent_populations_of_the_same_content_are_byte_identical() {
        let (workspace, _scheme_id) = one_scheme_workspace(3);
        let snapshot = workspace_document_snapshot(&workspace);

        let device_a = YrsJsonDocument::new(workspace.sync.id, SyncDocumentKind::PersonalWorkspace);
        device_a.populate(&snapshot).expect("device a populate");
        let device_b = YrsJsonDocument::new(workspace.sync.id, SyncDocumentKind::PersonalWorkspace);
        device_b.populate(&snapshot).expect("device b populate");

        assert_eq!(
            device_a.encode_state_v1(),
            device_b.encode_state_v1(),
            "two independent populations of identical content must be byte-identical"
        );
    }

    /// FIRST-SYNC IDENTITY RACE, part 2: the actual bug
    /// (`an_edit_made_while_a_sync_is_in_flight_is_pushed` in the desktop
    /// production fuzzer). Device A already populated the account's index from
    /// `base`. Device B populates from the SAME base, then edits (recolours)
    /// before ever syncing -- modeling "an edit made while the first sync is
    /// still in flight". B's edit must survive merging into A.
    #[test]
    fn an_edit_made_while_populating_from_a_shared_base_survives_merge() {
        let (base, scheme_id) = one_scheme_workspace(3);
        let base_snapshot = workspace_document_snapshot(&base);

        let device_a = YrsJsonDocument::new(base.sync.id, SyncDocumentKind::PersonalWorkspace);
        device_a
            .populate(&base_snapshot)
            .expect("device a populate");

        let device_b = YrsJsonDocument::new(base.sync.id, SyncDocumentKind::PersonalWorkspace);
        device_b
            .populate(&base_snapshot)
            .expect("device b populate");
        let mut edited = base.clone();
        edited.schemes.get_mut(&scheme_id).unwrap().color_index = 7;
        device_b
            .replace_snapshot(&workspace_document_snapshot(&edited))
            .expect("device b edit");

        // Merge B's state into A, as if B's edit had reached the account.
        device_a
            .apply_update_v1(&device_b.encode_state_v1())
            .expect("merge b into a");

        assert_eq!(
            colour_of(&device_a, scheme_id),
            7,
            "an edit made while populating from a shared base must survive the merge"
        );
    }

    /// FIRST-SYNC IDENTITY RACE, part 3: the PRECISE bug
    /// (`an_edit_made_while_a_sync_is_in_flight_is_pushed`) — unlike part 2
    /// above, device A and device B populate under DIFFERENT identities (each
    /// device's own per-install-random one), exactly as real installs do
    /// before either has adopted the account's canonical id. A plain re-key
    /// (`reidentify_workspace_document`) only rebinds the document; it cannot
    /// fix this, because the wrong-hashed population is still inside. Only
    /// `repopulate_canonically` — recomputing the population under the shared,
    /// canonical identity while preserving the edit made on top of the old
    /// one — can make the two sides' STARTER CONTENT actually deduplicate.
    #[test]
    fn devices_that_populated_under_different_pre_canonical_identities_still_converge() {
        let (base, scheme_id) = one_scheme_workspace(3);
        let base_snapshot = workspace_document_snapshot(&base);

        // Device A: already established the account under its OWN identity
        // (the common case — the first device to ever sync becomes canonical).
        let device_a = YrsJsonDocument::new(base.sync.id, SyncDocumentKind::PersonalWorkspace);
        device_a
            .populate(&base_snapshot)
            .expect("device a populate");

        // Device B: a DIFFERENT install, own random pre-sign-in identity —
        // populates from the identical logical content, but under a DIFFERENT
        // document id, so `populate`'s content hash (which embeds the id)
        // differs even though every other byte of the content is the same.
        let pre_canonical_id = DocumentId(uuid::Uuid::new_v4());
        let mut pre_canonical_base = base.clone();
        pre_canonical_base.sync.id = pre_canonical_id;
        let pre_canonical_snapshot = workspace_document_snapshot(&pre_canonical_base);
        let device_b = YrsJsonDocument::new(pre_canonical_id, SyncDocumentKind::PersonalWorkspace);
        device_b
            .populate(&pre_canonical_snapshot)
            .expect("device b populate under its own pre-sign-in identity");

        let mut edited = pre_canonical_base.clone();
        edited.schemes.get_mut(&scheme_id).unwrap().color_index = 7;
        device_b
            .replace_snapshot(&workspace_document_snapshot(&edited))
            .expect("device b edit (recolour) before its first sync lands");

        // Device B canonicalizes via `repopulate_canonically` instead of a
        // plain re-key, then merges into device A. `edited_canonical` is the
        // SAME edit, but canonicalized (sync.id swapped to the account's) —
        // mirroring what `canonicalize_personal_sync_identity_with_change`
        // does to the live model workspace before this is called for real.
        let mut edited_canonical = edited.clone();
        edited_canonical.sync.id = base.sync.id;
        let canonicalized = device_b
            .repopulate_canonically(
                &base_snapshot,
                &workspace_document_snapshot(&edited_canonical),
                base.sync.id,
            )
            .expect("repopulate canonically");
        device_a
            .apply_update_v1(&canonicalized.encode_state_v1())
            .expect("merge canonically-repopulated device b into device a");

        assert_eq!(
            colour_of(&device_a, scheme_id),
            7,
            "an edit made while populating under a pre-sign-in identity must \
             survive canonicalization and merge into the account"
        );
    }

    /// MIXED FLEET: a build predating `node_fields` writes only the whole-node
    /// entry, without the field-schema stamp. Its edit must win over the stale
    /// per-field keys it could not update.
    ///
    /// Without this, every updated device silently reverts the older device's
    /// rename/recolour and re-asserts the stale value on its next write, making
    /// the loss sticky. Desktop and mobile ship from separate release trains, so
    /// a mixed fleet is the normal state during a rollout.
    #[test]
    fn an_older_builds_whole_node_edit_wins_over_stale_field_keys() {
        let (doc, scheme_id) = indexed_workspace(3);

        // An older build recolours 3 -> 9: it regenerates the whole entry from
        // its own struct, so the entry carries no stamp and `node_fields` is
        // untouched.
        {
            let maps = WorkspaceMaps::get(&doc.doc);
            let mut txn = doc.doc.transact_mut();
            let key = scheme_id.to_string();
            let raw = maps
                .nodes
                .get_as::<_, Option<String>>(&txn, &key)
                .ok()
                .flatten()
                .expect("the scheme's node entry");
            let mut entry: serde_json::Value = serde_json::from_str(&raw).expect("node entry json");
            let object = entry.as_object_mut().expect("node entry object");
            object.remove("field_schema");
            let mut payload: serde_json::Value =
                serde_json::from_str(object["payload"].as_str().expect("payload string"))
                    .expect("payload json");
            payload["color_index"] = serde_json::json!(9);
            object.insert(
                "payload".to_string(),
                serde_json::Value::String(payload.to_string()),
            );
            maps.nodes.insert(
                &mut txn,
                key,
                serde_json::to_string(&entry).expect("re-encode node entry"),
            );
        }

        assert_eq!(
            colour_of(&doc, scheme_id),
            9,
            "an older build's recolour was shadowed by a stale node_fields key"
        );
    }

    /// The control for the test above: the per-field merge `node_fields` exists
    /// for must still work. A concurrent write from a build that DOES maintain
    /// the keys lands in `node_fields` while the whole-node payload keeps another
    /// device's value — there the field key is authoritative, exactly as before.
    ///
    /// Without this, the compatibility check above could be "passed" by never
    /// preferring the per-field keys at all, silently undoing the fix that
    /// introduced them.
    #[test]
    fn a_current_builds_field_write_still_wins_over_a_stale_payload() {
        let (doc, scheme_id) = indexed_workspace(3);

        // A concurrent current-build device recoloured to 7: its field key won,
        // while the whole-node payload still carries the other device's 3. The
        // entry keeps its stamp, because a current build wrote it.
        {
            let maps = WorkspaceMaps::get(&doc.doc);
            let mut txn = doc.doc.transact_mut();
            maps.node_fields.insert(
                &mut txn,
                node_field_key(&scheme_id.to_string(), "color_index"),
                "7".to_string(),
            );
        }

        assert_eq!(
            colour_of(&doc, scheme_id),
            7,
            "the per-field merge regressed: a current build's field write lost to \
             a stale whole-node payload"
        );
    }

    #[test]
    fn a_current_membership_field_wins_over_a_stale_whole_node_parent() {
        let (doc, scheme_id, destination) = indexed_workspace_with_folder();
        let scheme_key = scheme_id.to_string();

        // Simulate the whole-node half of a concurrent move retaining the old
        // root membership while the per-field move records the destination.
        {
            let maps = WorkspaceMaps::get(&doc.doc);
            let mut txn = doc.doc.transact_mut();
            let raw = maps
                .nodes
                .get_as::<_, Option<String>>(&txn, &scheme_key)
                .ok()
                .flatten()
                .expect("the scheme's node entry");
            let mut entry: WorkspaceNodeEntry =
                serde_json::from_str(&raw).expect("node entry json");
            entry.parent = String::new();
            maps.nodes.insert(
                &mut txn,
                scheme_key.clone(),
                serde_json::to_string(&entry).expect("re-encode node entry"),
            );
            maps.node_fields.insert(
                &mut txn,
                node_field_key(&scheme_key, "membership_parent"),
                destination.to_string(),
            );
        }

        let snapshot = doc.snapshot().expect("materialize merged index");
        let destination_folder = snapshot
            .folders
            .iter()
            .find(|folder| folder.id == destination)
            .expect("destination folder survives");
        assert!(
            destination_folder
                .children
                .contains(&NodeRef::Scheme(scheme_id)),
            "the current membership move was discarded in favor of the stale node entry"
        );
    }

    #[test]
    fn an_unloaded_live_scheme_keeps_its_index_node() {
        let (doc, scheme_id) = indexed_workspace(3);
        let mut loaded = doc.snapshot().expect("materialize initial index");
        loaded.schemes.clear();

        assert!(
            loaded
                .scheme_sync
                .iter()
                .any(|entry| entry.scheme == scheme_id),
            "the test workspace must retain the durable scheme binding"
        );
        doc.replace_snapshot(&loaded)
            .expect("rewrite index from a partial workspace");

        assert!(
            doc.snapshot()
                .expect("materialize retained index")
                .schemes
                .iter()
                .any(|scheme| scheme.id == scheme_id),
            "a live scheme binding must retain its node when its plain file is unloaded"
        );
    }

    #[test]
    fn folder_parent_materializes_from_merged_membership() {
        let mut workspace = Workspace::new();
        let parent_id = FolderId::new();
        let child_id = FolderId::new();
        workspace.folders.insert(
            parent_id,
            Folder {
                id: parent_id,
                name: "Parent".to_string(),
                parent: Some(workspace.root),
                children: vec![NodeRef::Folder(child_id)],
                expanded: true,
            },
        );
        workspace.folders.insert(
            child_id,
            Folder {
                id: child_id,
                name: "Child".to_string(),
                parent: Some(parent_id),
                children: Vec::new(),
                expanded: true,
            },
        );
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(parent_id));
        workspace.ensure_sync_metadata();
        let doc = YrsJsonDocument::new(workspace.sync.id, SyncDocumentKind::PersonalWorkspace);
        doc.replace_snapshot(&workspace_document_snapshot(&workspace))
            .expect("first index write");

        let child_key = child_id.to_string();
        let maps = WorkspaceMaps::get(&doc.doc);
        let mut txn = doc.doc.transact_mut();
        let raw = maps
            .nodes
            .get_as::<_, Option<String>>(&txn, &child_key)
            .ok()
            .flatten()
            .expect("child node entry");
        let mut entry: WorkspaceNodeEntry = serde_json::from_str(&raw).expect("node entry json");
        let mut payload: serde_json::Value =
            serde_json::from_str(&entry.payload).expect("folder payload json");
        payload["parent"] = serde_json::Value::String(workspace.root.to_string());
        entry.payload = payload.to_string();
        maps.nodes.insert(
            &mut txn,
            child_key,
            serde_json::to_string(&entry).expect("re-encode child node"),
        );
        drop(txn);

        let materialized = doc.snapshot().expect("materialize merged folder index");
        assert_eq!(
            materialized
                .folders
                .iter()
                .find(|folder| folder.id == child_id)
                .expect("child folder survives")
                .parent,
            Some(parent_id),
            "a stale redundant payload parent must not override membership"
        );
    }
}
