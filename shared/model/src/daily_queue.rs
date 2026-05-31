use chrono::NaiveDate;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{DocumentId, SchemeId, SyncDocumentKind, SyncDocumentMeta};

pub const DAILY_QUEUE_TITLE: &str = "Daily";
pub const DAILY_QUEUE_COLOR_INDEX: u8 = 0;
pub const PAGE_DAYS: i64 = 31;

const DAILY_QUEUE_SCHEME_NAMESPACE: [u8; 16] = [
    0x72, 0x38, 0x61, 0x5d, 0x6c, 0x7e, 0x46, 0x6f, 0x9f, 0x23, 0x91, 0xa8, 0xe5, 0x0a, 0xdf, 0x31,
];
const DAILY_QUEUE_DOCUMENT_NAMESPACE: [u8; 16] = [
    0x26, 0x69, 0x04, 0xa5, 0xd4, 0x2f, 0x4b, 0x37, 0x8c, 0x19, 0x58, 0x69, 0xbe, 0x1f, 0x45, 0x0c,
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
