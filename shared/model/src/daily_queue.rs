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
const DAILY_QUEUE_CARRYOVER_ITEM_NAMESPACE: [u8; 16] = [
    0x4b, 0x1a, 0x9d, 0x07, 0xe2, 0x55, 0x4c, 0x83, 0xa6, 0x71, 0x2f, 0x8c, 0x10, 0xd9, 0x3b, 0x6e,
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

/// Deterministic [`ItemId`] for a row carried into the daily queue for
/// `target_date`, derived from `(source_item_id, target_date)`. Carrying the same
/// source row into the same day twice — a double click, a retry after a sync
/// clobbered the optimistic insert, or a sync that re-creates a just-carried row —
/// always yields the SAME id. Identical ids let the carryover skip rows already
/// present (idempotent local insert) AND let the CRDT item-skeleton merge collapse
/// two devices' concurrent carries of the same row into one item instead of
/// doubling it. Mirrors [`daily_queue_scheme_id`]'s UUIDv8 derivation; uses its own
/// namespace so a carried id can never alias a scheme/document id.
pub fn daily_queue_carryover_item_id(source: ItemId, target_date: NaiveDate) -> ItemId {
    let name = format!("{}@{}", source.0, target_date);
    ItemId(stable_daily_uuid(DAILY_QUEUE_CARRYOVER_ITEM_NAMESPACE, &name))
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
