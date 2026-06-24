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
/// Namespace bit (bit 52) that PARTITIONS the 53-bit clientID space into two disjoint
/// halves: document/replica clientIDs (bit clear) and item-skeleton-seed clientIDs (bit
/// set). Both kinds are hashed into the same 53-bit space, so without this a document's
/// text-content struct could land on the SAME `(clientID, clock)` as an item skeleton's
/// struct — a silent id collision that makes the Yjs merge ORDER-DEPENDENT (the loser's
/// content is dropped on whichever side integrates second), permanently diverging
/// replicas. Reserving one bit per namespace makes the two kinds un-collidable.
const ITEM_SEED_NAMESPACE_BIT: u64 = 1 << 52;

/// Map a 64-bit hash into the document/replica clientID half: a 52-bit odd value with
/// the namespace bit CLEAR. `| 1` keeps it non-zero (and odd).
fn document_namespace_client_id(hash: u64) -> u64 {
    (hash & (ITEM_SEED_NAMESPACE_BIT - 1)) | 1
}

pub fn stable_client_id(replica_id: ReplicaId, document_id: DocumentId) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq.crdt.client_id.v1");
    hasher.update(replica_id.0.as_bytes());
    hasher.update(document_id.0.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    // Document/replica half of the partitioned space (namespace bit clear) so a
    // text-content struct can never alias an item-skeleton struct. See
    // [`ITEM_SEED_NAMESPACE_BIT`].
    document_namespace_client_id(u64::from_le_bytes(bytes))
}

/// A fresh random clientID in the document namespace (bit 52 clear), for documents
/// authored from an empty base. Stays in the same partition as [`stable_client_id`] so a
/// rebuilt document still never collides with an item-skeleton seed.
pub(crate) fn random_document_client_id() -> u64 {
    // v4 UUIDs are CSPRNG-backed; take 64 bits of that entropy (no extra rand dep).
    let (hi, _) = uuid::Uuid::new_v4().as_u64_pair();
    document_namespace_client_id(hi)
}

/// Deterministic Yjs clientID for an item's structural *skeleton*, derived from the
/// item id (NOT the replica). It is identical on every device, so two devices that
/// independently create the same item encode byte-identical creation ops and Yjs
/// dedupes them into one container instead of clobbering one (and discarding its
/// fields). A distinct hash namespace keeps it from colliding with any replica's
/// [`stable_client_id`]; device-specific *content* edits still use the replica
/// clientID, so concurrent edits stay distinct and merge (AB/BA) as before.
pub(crate) fn stable_item_seed_client_id(item_id: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq.crdt.item_seed_client_id.v1");
    hasher.update(item_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    // Item-skeleton half of the partitioned space (namespace bit SET) so a skeleton
    // struct can never alias a document's text-content struct. See
    // [`ITEM_SEED_NAMESPACE_BIT`]. `| 1` keeps it odd/non-zero.
    (u64::from_le_bytes(bytes) & (ITEM_SEED_NAMESPACE_BIT - 1)) | ITEM_SEED_NAMESPACE_BIT | 1
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
