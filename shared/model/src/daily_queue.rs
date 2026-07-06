use chrono::NaiveDate;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{DocumentId, ItemId, SchemeId, SyncDocumentKind, SyncDocumentMeta};

pub const DAILY_QUEUE_TITLE: &str = "Daily";
pub const DAILY_QUEUE_COLOR_INDEX: u8 = 0;
pub const PAGE_DAYS: i64 = 31;

const DAILY_QUEUE_SCHEME_NAMESPACE: [u8; 16] = [
    0x72, 0x38, 0x61, 0x5d, 0x6c, 0x7e, 0x46, 0x6f, 0x9f, 0x23, 0x91, 0xa8, 0xe5, 0x0a, 0xdf, 0x31,
];
const DAILY_QUEUE_DOCUMENT_NAMESPACE: [u8; 16] = [
    0x26, 0x69, 0x04, 0xa5, 0xd4, 0x2f, 0x4b, 0x37, 0x8c, 0x19, 0x58, 0x69, 0xbe, 0x1f, 0x45, 0x0c,
];
const DAILY_QUEUE_DISPLACED_ITEM_NAMESPACE: [u8; 16] = [
    0x9e, 0x5c, 0x2b, 0x41, 0x0f, 0x8a, 0x4d, 0x96, 0xb3, 0x27, 0x64, 0xd1, 0x7a, 0x0e, 0x58, 0xc2,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DailyQueueConfig;

impl DailyQueueConfig {
    pub const TITLE: &'static str = DAILY_QUEUE_TITLE;
    pub const COLOR_INDEX: u8 = DAILY_QUEUE_COLOR_INDEX;
    pub const PAGE_DAYS: i64 = PAGE_DAYS;
}

pub fn daily_queue_scheme_id(date: NaiveDate) -> SchemeId {
    SchemeId(stable_daily_uuid(
        DAILY_QUEUE_SCHEME_NAMESPACE,
        &date.to_string(),
    ))
}

pub fn daily_queue_document_id(date: NaiveDate) -> DocumentId {
    DocumentId(stable_daily_uuid(
        DAILY_QUEUE_DOCUMENT_NAMESPACE,
        &date.to_string(),
    ))
}

pub fn daily_queue_sync_metadata(date: NaiveDate) -> SyncDocumentMeta {
    let mut meta = SyncDocumentMeta::local(SyncDocumentKind::Scheme);
    meta.id = daily_queue_document_id(date);
    meta
}

/// Deterministic [`ItemId`] for the archived copy a carryover leaves behind on the
/// source day. Rolling a row forward MOVES its id to the new day (so notification
/// identity and cross-device dedupe follow the live item); the row left on
/// `source_date` is re-identified with this id, derived from
/// `(source_item_id, source_date)`. Determinism makes the displacement convergent:
/// two devices that concurrently roll the same day mint the SAME archived id, so
/// the CRDT item-skeleton merge collapses them into one row instead of doubling
/// yesterday. Includes the date so a row rolled forward day after day gets a
/// distinct archived id each day. Mirrors [`daily_queue_scheme_id`]'s UUIDv8
/// derivation; its own namespace keeps it from aliasing any other derived id.
pub fn daily_queue_displaced_item_id(source: ItemId, source_date: NaiveDate) -> ItemId {
    let name = format!("{}@{}", source.0, source_date);
    ItemId(stable_daily_uuid(
        DAILY_QUEUE_DISPLACED_ITEM_NAMESPACE,
        &name,
    ))
}

fn stable_daily_uuid(namespace: [u8; 16], name: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(namespace);
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // UUIDv8 leaves the payload application-defined while preserving RFC 9562
    // version/variant bits for tooling that expects UUID-shaped identifiers.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
