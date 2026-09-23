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
    // Minted through the model so the sync property fuzzer's deterministic id seed
    // covers clientIDs too — they are a merge input, so a run whose clientIDs came
    // from the OS CSPRNG could not be replayed from its seed. Production never sets
    // that seed, so this stays a fresh `Uuid::new_v4()` there.
    let (hi, _) = knotq_model::next_random_uuid().as_u64_pair();
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

/// Bump whenever what the first population of a scheme document writes changes —
/// which keys, in what order, how they are encoded. The population clientID is a
/// hash of the content, so two builds encoding the same content DIFFERENTLY under
/// the same clientID would reuse `(clientID, clock)` for different operations,
/// which Yjs cannot merge. `scheme_population_encoding_is_pinned` fails whenever
/// those bytes move, so the bump cannot be forgotten.
pub(crate) const SCHEME_POPULATION_ENCODING_VERSION: u32 = 3;

/// Deterministic clientID for the first population of an empty scheme document
/// from `content` (the serialized scheme it is populated with). Every replica
/// that populates `document` from identical content encodes byte-identical
/// operations under it, so Yjs integrates them once — rather than each install's
/// copy of the same fixed-id starter lines being inserted again beside the
/// others'. Different content hashes to a different clientID, so two different
/// populations never share an id. Document namespace: it authors text content.
/// Bump when the bytes an item CREATION encodes change (see
/// [`stable_item_creation_client_id`]): a build writing different bytes under the
/// same clientID as an older build would alias its structs.
pub(crate) const ITEM_CREATION_ENCODING_VERSION: u32 = 1;

/// Deterministic clientID for the initial text of a newly created item. Every
/// device that creates this item in this document with this exact content encodes
/// byte-identical text operations, so Yjs integrates them once instead of
/// concatenating one copy per device inside the (already deduped) Text. Different
/// content hashes to a different clientID, so two different creations stay
/// concurrent and merge as before. Document namespace: it authors text content.
pub(crate) fn stable_item_creation_client_id(
    document: DocumentId,
    item_id: &str,
    content: &[u8],
) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq.crdt.item_creation_client_id");
    hasher.update(ITEM_CREATION_ENCODING_VERSION.to_le_bytes());
    hasher.update(document.0.as_bytes());
    hasher.update(item_id.as_bytes());
    hasher.update((item_id.len() as u64).to_le_bytes());
    hasher.update(content);
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    document_namespace_client_id(u64::from_le_bytes(bytes))
}

pub(crate) fn stable_scheme_population_client_id(document: DocumentId, content: &[u8]) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq.crdt.scheme_population_client_id");
    hasher.update(SCHEME_POPULATION_ENCODING_VERSION.to_le_bytes());
    hasher.update(document.0.as_bytes());
    hasher.update(content);
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    document_namespace_client_id(u64::from_le_bytes(bytes))
}

/// Bump alongside [`WORKSPACE_POPULATION_ENCODING_VERSION`] whenever the bytes
/// [`YrsJsonDocument::replace_snapshot`] writes change — see
/// `SCHEME_POPULATION_ENCODING_VERSION` for why.
pub(crate) const WORKSPACE_POPULATION_ENCODING_VERSION: u32 = 1;

/// Deterministic clientID for the first population of an empty workspace-index
/// document from `content` (a serialized workspace snapshot). Every replica
/// that populates `document` from identical content encodes byte-identical
/// operations under it, so Yjs integrates them once instead of every node
/// becoming a genuinely concurrent write between two installs' independent
/// from-scratch populations of the SAME account content. Different content
/// hashes to a different clientID, so two different populations never share
/// an id. Document namespace: it authors map content, mirroring
/// `stable_scheme_population_client_id`.
pub(crate) fn stable_workspace_population_client_id(document: DocumentId, content: &[u8]) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq.crdt.workspace_population_client_id");
    hasher.update(WORKSPACE_POPULATION_ENCODING_VERSION.to_le_bytes());
    hasher.update(document.0.as_bytes());
    hasher.update(content);
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    document_namespace_client_id(u64::from_le_bytes(bytes))
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

/// `update_v1` with every delete-set entry that names a struct `known` already
/// holds removed, and everything else — every struct and every other tombstone
/// — kept byte for byte.
///
/// A full-state update (`encode_state_v1`) carries the document's entire delete
/// set, and the delete set is the one part of an update that acts on structs the
/// *receiver* holds rather than on structs the update carries. That is exactly
/// what must not cross an account boundary: two accounts hold byte-identical
/// structs for the same derived document id (a Daily page's starter rows, a
/// fixed-id scheme's lines, anything a device carried across before), so a
/// tombstone one account authored deletes the other account's live row. A
/// struct the update *carries* is harmless on its own — one the receiver has is
/// skipped as already integrated, and one it lacks arrives with its deletion
/// baked in (`ItemContent::Deleted` / GC blocks self-delete on integration) —
/// so the structs go through unchanged and only the delete set is narrowed to
/// clocks past `known`, where the receiver has nothing and a tombstone can only
/// apply to the structs travelling beside it.
///
/// Implemented on the wire encoding: yrs writes the delete set as the tail of a
/// v1 update, so the tail is re-encoded from the filtered set and everything
/// before it is kept verbatim.
pub(crate) fn update_v1_without_deletes_known_to(
    update_v1: &[u8],
    known: &StateVector,
) -> anyhow::Result<Vec<u8>> {
    use yrs::ID;
    let update = Update::decode_v1(update_v1)?;
    let full = update.encode_v1();
    let delete_set_tail = update.delete_set().encode_v1();
    anyhow::ensure!(
        full.ends_with(&delete_set_tail),
        "yrs update encoding no longer ends with its delete set"
    );
    let mut kept = yrs::IdSet::new();
    for (client, ranges) in update.delete_set().iter() {
        let known_clock = known.get(client);
        for range in ranges.iter() {
            let start = range.start.max(known_clock);
            if start < range.end {
                kept.insert(ID::new(*client, start), range.end - start);
            }
        }
    }
    let mut out = full[..full.len() - delete_set_tail.len()].to_vec();
    out.extend(kept.encode_v1());
    Ok(out)
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
