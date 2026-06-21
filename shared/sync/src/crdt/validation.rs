//! CRDT schema validation: re-runs the server's structural checks on workspace and
//! scheme documents before anything is materialized or persisted.
use super::*;

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

pub(crate) fn validate_workspace_document(doc: &Doc) -> anyhow::Result<()> {
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
    root.parse::<uuid::Uuid>()
        .context("workspace root invalid")?;
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

pub(crate) fn validate_scheme_document(doc: &Doc) -> anyhow::Result<()> {
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

pub(crate) fn validate_scheme_item(
    item_id: &str,
    parsed_item_id: ItemId,
    item_map: &MapRef,
    txn: &impl ReadTxn,
) -> anyhow::Result<()> {
    let str_field = |key: &str| {
        item_map
            .get_as::<_, Option<String>>(txn, key)
            .ok()
            .flatten()
    };
    let require_str =
        |key: &str| str_field(key).ok_or_else(|| anyhow!("item {key} missing: {item_id}"));

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
    if ItemMarker::parse(&require_str("marker")?).is_err() {
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
    parse_json_value(&require_str("repeats_json")?)
        .with_context(|| format!("item repeats invalid: {item_id}"))?;
    parse_json_value(&require_str("state_json")?)
        .with_context(|| format!("item state invalid: {item_id}"))?;
    parse_json_value(&require_str("priority_json")?)
        .with_context(|| format!("item priority invalid: {item_id}"))?;
    parse_json_value(&require_str("external_json")?)
        .with_context(|| format!("item external invalid: {item_id}"))?;

    // Content is a collaborative rich-text sequence (yrs Text): text lives as
    // normal string chunks and images/tables live as embedded JSON blobs.
    if item_text_ref(item_map, txn).is_none() {
        return Err(anyhow!("item text missing or not a text type: {item_id}"));
    }

    // snapshot_json carries non-content metadata. The ordered inline content
    // lives in the Text CRDT and is intentionally absent here.
    let snapshot: Item = serde_json::from_str(&require_str("snapshot_json")?)
        .with_context(|| format!("item snapshot invalid: {item_id}"))?;
    if snapshot.id != parsed_item_id {
        return Err(anyhow!("item snapshot id mismatch: {item_id}"));
    }

    Ok(())
}

pub(crate) fn parse_optional_rfc3339(value: &str) -> anyhow::Result<()> {
    if !value.is_empty() {
        DateTime::parse_from_rfc3339(value)?;
    }
    Ok(())
}

pub(crate) fn parse_json_value(value: &str) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::from_str(value)?)
}
