//! Low-level CRDT encoding helpers: stable client IDs, Yjs document options, and
//! inline-embed (image/table) serialization. Behavior-identical to the original
//! inline definitions in `crdt.rs`.
use super::*;

/// Derive a stable 64-bit Yjs clientID for a document on a given replica. Two
/// different replicas get different clientIDs (so their concurrent map writes are
/// ordered deterministically by last-writer-wins); the same replica always gets the
/// same clientID for a document, so re-encoding persisted state never aliases under a
/// fresh identity. Per-document (not just per-replica) keeps independent documents
/// from sharing an op space.
pub fn stable_client_id(replica_id: ReplicaId, document_id: DocumentId) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq.crdt.client_id.v1");
    hasher.update(replica_id.0.as_bytes());
    hasher.update(document_id.0.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    // yrs/Yjs clientIDs are 53-bit (JS safe-integer) — mask to that range. `| 1`
    // keeps it non-zero. The 53-bit space is ample for the handful of replicas on an
    // account, so collisions are negligible.
    (u64::from_le_bytes(bytes) & ((1u64 << 53) - 1)) | 1
}

pub(crate) fn encode_inline_embed(inline: &Inline) -> anyhow::Result<String> {
    Ok(format!(
        "{INLINE_EMBED_PREFIX}{}",
        serde_json::to_string(inline)?
    ))
}

pub(crate) fn decode_inline_embed_str(text: &str) -> Option<Inline> {
    text.strip_prefix(INLINE_EMBED_PREFIX)
        .and_then(|json| serde_json::from_str::<Inline>(json).ok())
        .and_then(|inline| (!inline.is_text()).then_some(inline))
}

pub(crate) fn serde_json_string_value(value: &impl Serialize) -> anyhow::Result<String> {
    let value = serde_json::to_value(value)?;
    Ok(value.as_str().unwrap_or_default().to_string())
}

/// True when `update_v1` carries no operations. A no-op Yjs diff is not
/// zero-length: it encodes as the canonical 2-byte update `[0, 0]` (zero struct
/// clients, zero delete-set clients). Treating it as a real update queues no-op
/// pushes — and for a brand-new empty document it is the *only* update, which the
/// backend rejects as `crdt_schema_invalid`.
pub(crate) fn update_v1_is_empty(update_v1: &[u8]) -> bool {
    update_v1.is_empty() || update_v1 == [0, 0]
}

pub(crate) fn yrs_doc_options(id: DocumentId, client_id: u64, offset_kind: OffsetKind) -> Options {
    let mut options =
        Options::with_guid_and_client_id(id.0.to_string().into(), ClientID::new(client_id));
    options.offset_kind = offset_kind;
    options
}
