//! The per-scheme content CRDT (`YrsSchemeDocument`): item storage as id-keyed
//! maps, rich-text content as a Yjs Text sequence, and the minimal-splice diffing
//! that keeps ordinary character edits convergent.
use super::*;

/// Create the scheme document's two root maps. Both constructors seed identical
/// structure, so they share this to stay byte-compatible.
fn init_scheme_maps(doc: &Doc) {
    doc.get_or_insert_map("scheme_file");
    doc.get_or_insert_map("items_by_id");
}

pub struct YrsSchemeDocument {
    pub(crate) id: DocumentId,
    doc: Doc,
    encode_cache: EncodeCache,
}

impl YrsSchemeDocument {
    pub fn new(id: DocumentId) -> Self {
        // UTF-16 offsets match Yjs (JS) semantics, so the text-diff index math here lines
        // up with any future JavaScript collaboration client and never splits a multi-byte
        // character. The clientID is a fresh random value in the DOCUMENT namespace (not
        // yrs's unpartitioned default), so a from-empty rebuild can never alias an
        // item-skeleton seed clientID — see `stable_item_seed_client_id`.
        let doc = Doc::with_options(yrs_doc_options(
            id,
            random_document_client_id(),
            OffsetKind::Utf16,
        ));
        init_scheme_maps(&doc);
        let encode_cache = EncodeCache::new(&doc);
        Self {
            id,
            doc,
            encode_cache,
        }
    }

    pub(crate) fn new_with_client_id(id: DocumentId, client_id: u64) -> Self {
        let doc = Doc::with_options(yrs_doc_options(id, client_id, OffsetKind::Utf16));
        init_scheme_maps(&doc);
        let encode_cache = EncodeCache::new(&doc);
        Self {
            id,
            doc,
            encode_cache,
        }
    }

    /// Build a scheme document whose clientID is either deterministic for
    /// `replica_id` (stable across reconstructions) or random when `None`.
    pub(crate) fn for_replica(id: DocumentId, replica_id: Option<ReplicaId>) -> Self {
        match replica_id {
            Some(replica) => Self::new_with_client_id(id, stable_client_id(replica, id)),
            None => Self::new(id),
        }
    }

    /// Full document state as a v1 update, for durable persistence. Cached: the
    /// document is only re-serialized when it changed since the last call.
    pub fn encode_state_v1(&self) -> Vec<u8> {
        self.encode_cache
            .get(|| self.doc.transact().encode_diff_v1(&StateVector::default()))
    }

    pub fn from_scheme(id: DocumentId, scheme: &Scheme) -> anyhow::Result<Self> {
        let this = Self::new(id);
        this.replace_scheme(scheme)?;
        Ok(this)
    }

    pub fn sync_scheme(&self, scheme: &Scheme) -> anyhow::Result<Option<CrdtDocumentUpdate>> {
        let before = self.state_vector_v1();
        let touched = self.replace_scheme(scheme)?;
        let update_v1 = self.encode_update_v1(&before)?;
        if update_v1_is_empty(&update_v1) {
            return Ok(None);
        }
        let mut touched_items: Vec<String> = touched.into_iter().collect();
        touched_items.sort();
        Ok(Some(CrdtDocumentUpdate {
            document: self.id,
            kind: SyncDocumentKind::Scheme,
            update_v1,
            touched_items,
        }))
    }

    /// Deterministically create the skeleton (sub-map + `schema`/`id` + empty Text) for
    /// every item not yet in the document, under a fixed clientID derived from the item
    /// id. Identical across devices, so two independent creations of the same item
    /// dedupe into one container instead of clobbering one. Each skeleton is applied as
    /// its own sub-update (each uses its own id-derived clientID) before the main edit
    /// transaction.
    fn ensure_item_skeletons(&self, scheme: &Scheme) -> anyhow::Result<()> {
        let items_by_id = self.doc.get_or_insert_map("items_by_id");
        let missing: Vec<String> = {
            let txn = self.doc.transact();
            scheme
                .items
                .iter()
                .map(|item| item.id.to_string())
                .filter(|id| item_map_ref(&items_by_id, &txn, id).is_none())
                .collect()
        };
        for item_id in missing {
            let skeleton = build_item_skeleton_update(self.id, &item_id);
            self.doc
                .transact_mut()
                .apply_update(Update::decode_v1(&skeleton)?)?;
        }
        Ok(())
    }

    /// Make the document match `scheme` exactly, returning the ids of the items
    /// the call actually wrote (created, content-spliced, metadata-rewritten, or
    /// tombstoned) — i.e. the items the resulting update touches.
    pub fn replace_scheme(&self, scheme: &Scheme) -> anyhow::Result<HashSet<String>> {
        let mut touched: HashSet<String> = HashSet::new();
        // Deterministically create the skeleton for any new item BEFORE the main edit
        // transaction, so two devices that independently create the same item dedupe
        // into one container instead of clobbering one. The content/metadata writes
        // below then fill those skeletons under the device clientID (and so still merge
        // AB/BA). Applied as sub-updates because each item's skeleton uses its own
        // id-derived clientID.
        self.ensure_item_skeletons(scheme)?;
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
            // A skeleton-created item has no real position yet (read back as ""); treat
            // an empty position as absent so a fresh fractional key is minted.
            let existing = stored
                .get(id)
                .map(|entry| entry.position.clone())
                .filter(|position| !position.is_empty());
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
            let Some(item_map) = item_map_ref(&items_by_id, &txn, &key) else {
                continue;
            };
            touched.insert(key.clone());
            let has_schema = item_map
                .get_as::<_, Option<String>>(&txn, "schema")
                .ok()
                .flatten()
                .is_some();
            if has_schema {
                // A valid item the user removed → tombstone (soft-delete). A hard remove
                // concurrent with another replica's edit detaches the map and loses
                // fields ("item schema missing"), wedging the scheme; a tombstone keeps a
                // valid map that merges with concurrent edits. Materialization skips it.
                item_map.insert(&mut txn, "deleted", true);
            } else {
                // Already a partial/clobbered entry (schema missing, e.g. from a legacy
                // hard-remove race). Hard-remove it: tombstoning would leave it
                // schema-invalid and the backend would reject the push. Dropping converges
                // — every replica sees the same partial in the merged state and drops it.
                items_by_id.remove(&mut txn, &key);
            }
        }

        // For each item, merge content as a rich-text sequence CRDT and treat
        // non-content fields as last-writer-wins metadata:
        //   - new item         -> insert the full entry (content seeded into a Text type)
        //   - content changed   -> splice the minimal changed range into the Text so
        //                          ordinary character edits converge
        //   - metadata changed  -> rewrite the scalar fields + metadata blob only
        // so a content edit never recreates (and clobbers) the collaborative Text.
        for (item, position) in scheme.items.iter().zip(&positions) {
            let item_id = item.id.to_string();
            let next_snapshot = item_snapshot_json(item)?;
            let prev = stored.get(&item_id);
            match item_map_ref(&items_by_id, &txn, &item_id) {
                None => {
                    touched.insert(item_id.clone());
                    let item_map = items_by_id.insert(&mut txn, item_id, MapPrelim::default());
                    write_new_item(&item_map, &mut txn, item, position, &next_snapshot)?;
                }
                Some(item_map) => {
                    match item_text_ref(&item_map, &txn) {
                        Some(text_ref) => {
                            let current = match prev {
                                Some(stored) => stored.content.clone(),
                                None => read_text_content(&text_ref, &txn),
                            };
                            let new_content = normalize_inline_content(&item.content.to_inlines());
                            if current != new_content {
                                touched.insert(item_id.clone());
                                apply_content_diff(&text_ref, &mut txn, &current, &new_content)?;
                            }
                            if prev.is_none_or(|stored| {
                                stored.content_shadow.as_deref() != Some(new_content.as_slice())
                            }) {
                                touched.insert(item_id.clone());
                                write_item_content_shadow(&item_map, &mut txn, &new_content)?;
                            }
                        }
                        // No Text present (an entry that lost its Text). Seed a fresh
                        // Text IN PLACE and fall through to write the metadata fields. Do
                        // NOT re-insert the items_by_id entry: a re-insert detaches the
                        // whole map and races a concurrent edit into a schema-less "only
                        // text" partial — the exact clobber we are eliminating. The entry
                        // is a materialized item, so its immutable schema/id already
                        // exist; only the Text needs rebuilding.
                        None => {
                            touched.insert(item_id.clone());
                            let new_content = normalize_inline_content(&item.content.to_inlines());
                            let text_ref = item_map.insert(&mut txn, "text", TextPrelim::new(""));
                            insert_inline_content(&text_ref, &mut txn, &new_content)?;
                            write_item_content_shadow(&item_map, &mut txn, &new_content)?;
                        }
                    }
                    // `stored.deleted` participates: a tombstoned entry being re-added
                    // with byte-identical snapshot+position (undo of a delete) still
                    // needs the metadata write, whose `deleted=false` un-tombstones it.
                    // Skipping it would leave the doc (and every peer) considering the
                    // item deleted while the local workspace shows it alive.
                    let metadata_changed = prev.is_none_or(|stored| {
                        stored.snapshot_json != next_snapshot
                            || stored.position != *position
                            || stored.deleted
                    });
                    if metadata_changed {
                        touched.insert(item_id.clone());
                        write_item_metadata(&item_map, &mut txn, item, position, &next_snapshot)?;
                    }
                }
            }
        }
        Ok(touched)
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

    /// Applies a remote update and reports whether it changed this document.
    /// An echo of state this replica already holds (e.g. the server broadcasting
    /// our own push back) merges as a no-op; comparing the (state vector,
    /// delete set) snapshot before/after detects that — including delete-only
    /// updates, which advance no clock.
    pub fn apply_update_v1(&self, update: &[u8]) -> anyhow::Result<bool> {
        let mut txn = self.doc.transact_mut();
        let before = txn.snapshot();
        txn.apply_update(Update::decode_v1(update)?)?;
        Ok(txn.snapshot() != before)
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        validate_scheme_document(&self.doc)
    }

    /// All stored item entries sorted by `(position, item_id)`. The id breaks
    /// ties so replicas that independently generated the same fractional key
    /// still converge to one deterministic order.
    pub(crate) fn sorted_entries(&self) -> anyhow::Result<Vec<(String, StoredItem)>> {
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
            .map(|(_, entry)| inline_text(&entry.content))
            .collect())
    }

    pub(crate) fn scheme_items(&self) -> anyhow::Result<Vec<Item>> {
        Ok(self
            .sorted_entries()?
            .into_iter()
            // Skip tombstoned items, and any partial entry (empty snapshot) left by a
            // pre-tombstone concurrent remove/edit clobber, so materialization stays
            // consistent across replicas instead of failing the whole scheme.
            .filter(|(_, entry)| !entry.deleted && !entry.snapshot_json.is_empty())
            .filter_map(|(_, entry)| {
                // snapshot_json holds every non-content field; the ordered inline
                // stream comes from the Text CRDT, which is the source of truth.
                // Tolerate a partial: a snapshot that fails to parse is skipped rather
                // than failing the whole scheme load. Every replica holds the same
                // merged CRDT, so each skips the same item and they converge — matching
                // the server's tolerant validation (see validate_scheme_document).
                let mut item: Item = serde_json::from_str(&entry.snapshot_json).ok()?;
                item.content = ItemContent::from_inlines(entry.content);
                Some(item)
            })
            .collect())
    }
}

/// What we need from a stored item entry without deserializing the whole nested
/// map (its `text` is a Text type, not a scalar serde can read).
pub(crate) struct StoredItem {
    position: String,
    snapshot_json: String,
    content: Vec<Inline>,
    content_shadow: Option<Vec<Inline>>,
    /// Soft-delete tombstone. A removed item keeps its (valid) map with `deleted=true`
    /// instead of being hard-removed, so a concurrent remove+edit can't detach the map
    /// and lose fields. Materialization skips tombstoned items.
    deleted: bool,
}

pub(crate) fn item_map_ref(items_by_id: &MapRef, txn: &impl ReadTxn, key: &str) -> Option<MapRef> {
    match items_by_id.get(txn, key) {
        Some(Out::YMap(map)) => Some(map),
        _ => None,
    }
}

pub(crate) fn item_text_ref(item_map: &MapRef, txn: &impl ReadTxn) -> Option<TextRef> {
    match item_map.get(txn, "text") {
        Some(Out::YText(text)) => Some(text),
        _ => None,
    }
}

/// Build the deterministic "create this item's skeleton" sub-update: the item's
/// sub-map with invariant `schema`/`id` and an empty Text, encoded under a fixed
/// clientID derived from `item_id` (see [`stable_item_seed_client_id`]). Byte-identical
/// across devices for a given id, so applying two independent creations dedupes to a
/// single container rather than clobbering one and losing its fields.
fn build_item_skeleton_update(document: DocumentId, item_id: &str) -> Vec<u8> {
    let doc = Doc::with_options(yrs_doc_options(
        document,
        stable_item_seed_client_id(item_id),
        OffsetKind::Utf16,
    ));
    {
        let items = doc.get_or_insert_map("items_by_id");
        let mut txn = doc.transact_mut();
        let item_map = items.insert(&mut txn, item_id, MapPrelim::default());
        item_map.insert(&mut txn, "schema", "knotq.item.v1");
        item_map.insert(&mut txn, "id", item_id);
        item_map.insert(&mut txn, "text", TextPrelim::new(""));
    }
    let update = doc.transact().encode_diff_v1(&StateVector::default());
    update
}

pub(crate) fn read_stored_item(item_map: &MapRef, txn: &impl ReadTxn) -> StoredItem {
    let str_field = |key: &str| {
        item_map
            .get_as::<_, Option<String>>(txn, key)
            .ok()
            .flatten()
            .unwrap_or_default()
    };
    let content_shadow = serde_json::from_str::<Vec<Inline>>(&str_field("content_json")).ok();
    let content = item_text_ref(item_map, txn)
        .map(|text| read_text_content(&text, txn))
        .unwrap_or_default();
    StoredItem {
        position: str_field("position"),
        snapshot_json: str_field("snapshot_json"),
        content: reconcile_content_shadow(content, content_shadow.as_deref()),
        content_shadow,
        deleted: item_map
            .get_as::<_, Option<bool>>(txn, "deleted")
            .ok()
            .flatten()
            .unwrap_or(false),
    }
}

/// Serialize every item field except content. Content is owned by the Text CRDT,
/// so keeping it out of the snapshot blob means a text/embed edit never rewrites
/// the blob and the two representations cannot disagree.
pub(crate) fn item_snapshot_json(item: &Item) -> anyhow::Result<String> {
    let mut snapshot = item.clone();
    snapshot.content = ItemContent::default();
    Ok(serde_json::to_string(&snapshot)?)
}

pub(crate) fn write_new_item(
    item_map: &MapRef,
    txn: &mut TransactionMut,
    item: &Item,
    position: &str,
    snapshot_json: &str,
) -> anyhow::Result<()> {
    write_item_fields(item_map, txn, item, position, snapshot_json, true)?;
    // content_json is owned by the shadow writer (see write_item_fields' note), so the
    // creation path writes it here — exactly once.
    write_item_content_shadow(
        item_map,
        txn,
        &normalize_inline_content(&item.content.to_inlines()),
    )
}

/// Rewrite an existing item's last-writer-wins metadata fields in place, leaving
/// its collaborative Text untouched.
pub(crate) fn write_item_metadata(
    item_map: &MapRef,
    txn: &mut TransactionMut,
    item: &Item,
    position: &str,
    snapshot_json: &str,
) -> anyhow::Result<()> {
    write_item_fields(item_map, txn, item, position, snapshot_json, false)
}

pub(crate) fn write_item_content_shadow(
    item_map: &MapRef,
    txn: &mut TransactionMut,
    content: &[Inline],
) -> anyhow::Result<()> {
    item_map.insert(txn, "content_json", serde_json::to_string(content)?);
    Ok(())
}

pub(crate) fn write_item_fields(
    item_map: &MapRef,
    txn: &mut TransactionMut,
    item: &Item,
    position: &str,
    snapshot_json: &str,
    include_text: bool,
) -> anyhow::Result<()> {
    // `schema` and `id` are immutable identity fields. They are written once at
    // creation — by the deterministic item skeleton (shared seed clientID, so every
    // origin that creates this item produces the identical, de-duplicated struct) or by
    // the rebuild paths below. We deliberately DO NOT re-write them on a metadata-only
    // edit: re-inserting a map key replaces its struct, so concurrent metadata edits
    // from different origins each delete the other's `schema`/`id` copy, and a merge can
    // end up deleting EVERY copy — leaving a schema-less "only text" partial that fails
    // validation and wedges the scheme. Writing them only at creation keeps a single
    // stable struct that no edit ever churns.
    if include_text {
        item_map.insert(txn, "schema", "knotq.item.v1");
        item_map.insert(txn, "id", item.id.to_string());
    }
    item_map.insert(txn, "position", position.to_string());
    // A live item is not tombstoned. Setting this also un-deletes an item that was
    // re-added after a soft-delete (deleted vs edit resolves last-writer-wins).
    item_map.insert(txn, "deleted", false);
    if include_text {
        let text_ref = item_map.insert(txn, "text", TextPrelim::new(""));
        insert_inline_content(
            &text_ref,
            txn,
            &normalize_inline_content(&item.content.to_inlines()),
        )?;
    }
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
    item_map.insert(txn, "repeats_json", serde_json::to_string(&item.repeats)?);
    item_map.insert(txn, "state_json", serde_json::to_string(&item.state)?);
    item_map.insert(txn, "priority_json", serde_json::to_string(&item.priority)?);
    item_map.insert(txn, "external_json", serde_json::to_string(&item.external)?);
    item_map.insert(txn, "snapshot_json", snapshot_json.to_string());
    // NOTE: `content_json` (the content shadow) is intentionally NOT written here — it is
    // owned solely by `write_item_content_shadow`. Writing it both here and there inserts
    // the same key twice in one creation transaction; the second insert tombstones the
    // first, baking a permanent delete into the document that `encode_diff_v1` re-emits on
    // every subsequent reconcile — a spurious, non-idempotent edit on unchanged content.
    Ok(())
}

pub(crate) fn read_text_content(text: &TextRef, txn: &impl ReadTxn) -> Vec<Inline> {
    let mut content = Vec::new();
    for diff in text.diff(txn, |_| ()) {
        if let Out::Any(Any::String(text)) = diff.insert {
            let text = text.as_ref();
            if let Some(inline) = decode_inline_embed_str(text) {
                content.push(inline);
            } else {
                push_text_inline(&mut content, text);
            }
        }
    }
    normalize_inline_content(&content)
}

pub(crate) fn normalize_inline_content(content: &[Inline]) -> Vec<Inline> {
    let mut normalized = Vec::with_capacity(content.len());
    for inline in content {
        match inline {
            Inline::Text { text } => push_text_inline(&mut normalized, text),
            Inline::Image(image) => normalized.push(Inline::Image(*image)),
            Inline::Table(table) => normalized.push(Inline::Table(table.clone())),
        }
    }
    normalized
}

pub(crate) fn push_text_inline(content: &mut Vec<Inline>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text { text: previous }) = content.last_mut() {
        previous.push_str(text);
    } else {
        content.push(Inline::Text {
            text: text.to_string(),
        });
    }
}

pub(crate) fn inline_text(content: &[Inline]) -> String {
    let mut text = String::new();
    for inline in content {
        if let Inline::Text { text: chunk } = inline {
            text.push_str(chunk);
        }
    }
    text
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ContentUnit {
    Char(char),
    Embed(Inline),
}

pub(crate) fn content_units(content: &[Inline]) -> Vec<ContentUnit> {
    let mut units = Vec::new();
    for inline in content {
        match inline {
            Inline::Text { text } => units.extend(text.chars().map(ContentUnit::Char)),
            Inline::Image(image) => units.push(ContentUnit::Embed(Inline::Image(*image))),
            Inline::Table(table) => units.push(ContentUnit::Embed(Inline::Table(table.clone()))),
        }
    }
    units
}

pub(crate) fn unit_len(unit: &ContentUnit) -> u32 {
    match unit {
        ContentUnit::Char(ch) => ch.len_utf16() as u32,
        ContentUnit::Embed(_) => 1,
    }
}

pub(crate) fn units_len(units: &[ContentUnit]) -> u32 {
    units.iter().map(unit_len).sum()
}

pub(crate) fn reconcile_content_shadow(
    content: Vec<Inline>,
    shadow: Option<&[Inline]>,
) -> Vec<Inline> {
    let Some(shadow) = shadow else {
        return content;
    };
    let shadow = normalize_inline_content(shadow);
    if content == shadow {
        return content;
    }
    let actual_units = content_units(&content);
    let shadow_units = content_units(&shadow);
    if !shadow_units.is_empty()
        && actual_units.len() == shadow_units.len() * 2
        && actual_units[..shadow_units.len()] == shadow_units
        && actual_units[shadow_units.len()..] == shadow_units
    {
        return shadow;
    }
    content
}

/// Apply the change from `old` to `new` as a single contiguous splice on the
/// rich Text (the common prefix and suffix are left untouched). Text characters
/// remain collaborative, while image/table embeds move with their surrounding
/// content as first-class sequence elements.
pub(crate) fn apply_content_diff(
    text: &TextRef,
    txn: &mut TransactionMut,
    old: &[Inline],
    new: &[Inline],
) -> anyhow::Result<()> {
    if old == new {
        return Ok(());
    }
    let old_units = content_units(old);
    let new_units = content_units(new);
    let min_len = old_units.len().min(new_units.len());
    let mut prefix = 0;
    while prefix < min_len && old_units[prefix] == new_units[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < (min_len - prefix)
        && old_units[old_units.len() - 1 - suffix] == new_units[new_units.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let at = units_len(&old_units[..prefix]);
    let removed = units_len(&old_units[prefix..old_units.len() - suffix]);
    if removed > 0 {
        text.remove_range(txn, at, removed);
    }
    insert_units(text, txn, at, &new_units[prefix..new_units.len() - suffix])?;
    Ok(())
}

pub(crate) fn insert_inline_content(
    text: &TextRef,
    txn: &mut TransactionMut,
    content: &[Inline],
) -> anyhow::Result<()> {
    insert_units(text, txn, 0, &content_units(content))
}

pub(crate) fn insert_units(
    text: &TextRef,
    txn: &mut TransactionMut,
    mut at: u32,
    units: &[ContentUnit],
) -> anyhow::Result<()> {
    let mut pending = String::new();
    for unit in units {
        match unit {
            ContentUnit::Char(ch) => pending.push(*ch),
            ContentUnit::Embed(inline) => {
                if !pending.is_empty() {
                    text.insert(txn, at, &pending);
                    at += pending.chars().map(|ch| ch.len_utf16() as u32).sum::<u32>();
                    pending.clear();
                }
                text.insert_embed(txn, at, encode_inline_embed(inline)?);
                at += 1;
            }
        }
    }
    if !pending.is_empty() {
        text.insert(txn, at, &pending);
    }
    Ok(())
}
