use serde::{Deserialize, Serialize};

use crate::daily_queue::stable_derived_uuid;
use crate::{DocumentId, FolderId, SchemeId, ShareId};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncDocumentMeta {
    #[serde(default)]
    pub id: DocumentId,
    pub kind: SyncDocumentKind,
    #[serde(default)]
    pub crdt: CrdtBackend,
    #[serde(default)]
    pub access: SyncAccess,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteDocumentRef>,
}

impl SyncDocumentMeta {
    pub fn local(kind: SyncDocumentKind) -> Self {
        Self {
            id: DocumentId::new(),
            kind,
            crdt: CrdtBackend::Yrs,
            access: SyncAccess::Local,
            remote: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncDocumentKind {
    PersonalWorkspace,
    Scheme,
    Folder,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrdtBackend {
    OperationLog,
    #[default]
    Yrs,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum SyncAccess {
    #[default]
    Local,
    Private,
    Shared {
        share: ShareId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteDocumentRef {
    pub server: String,
    pub remote_id: String,
}

pub fn default_workspace_sync() -> SyncDocumentMeta {
    SyncDocumentMeta::local(SyncDocumentKind::PersonalWorkspace)
}

const SCHEME_CONTENT_DOCUMENT_NAMESPACE: [u8; 16] = [
    0x4b, 0xd7, 0x12, 0x9c, 0x83, 0x5e, 0x4a, 0x21, 0xa6, 0x0d, 0x37, 0xf4, 0x2b, 0x91, 0x6e, 0x58,
];

/// Deterministic content-document id for a scheme, derived from the scheme id.
///
/// The scheme→document binding lives in the workspace CRDT document and can be
/// transiently dropped when a scheme is absent from one device's materialized
/// workspace (cross-account carries, pull ordering) — `ensure_sync_metadata`
/// then re-mints it. A RANDOM re-mint let every device bind the same scheme to
/// a different fresh document: the bindings raced in the workspace-doc merge,
/// the winner pointed at an empty document, and the scheme's real content doc
/// became an unreachable orphan (silent loss for fresh pullers). Deriving the
/// id from the scheme id makes every (re-)mint on every device converge on the
/// SAME document, so a dropped binding is always reconstructed identically —
/// the same convergence-by-derivation pattern as `daily_queue_document_id` and
/// `stable_item_seed_client_id`.
pub fn scheme_content_document_id(scheme: SchemeId) -> DocumentId {
    DocumentId(stable_derived_uuid(
        SCHEME_CONTENT_DOCUMENT_NAMESPACE,
        scheme.0.as_bytes(),
    ))
}

/// Sync metadata for a scheme's content document with the deterministic
/// [`scheme_content_document_id`] binding. The only way scheme metadata should
/// be minted — a random binding here reintroduces divergent re-minting.
pub fn scheme_content_sync_metadata(scheme: SchemeId) -> SyncDocumentMeta {
    let mut meta = SyncDocumentMeta::local(SyncDocumentKind::Scheme);
    meta.id = scheme_content_document_id(scheme);
    meta
}

const FOLDER_DOCUMENT_NAMESPACE: [u8; 16] = [
    0x1c, 0x64, 0xaf, 0x02, 0x7d, 0x39, 0x4e, 0x8b, 0x95, 0x21, 0xc8, 0x0a, 0x63, 0xde, 0x47, 0x15,
];

/// Deterministic document id for a folder, derived from the folder id — the
/// folder counterpart of [`scheme_content_document_id`], and for the same
/// reason.
///
/// A folder's binding is re-minted by `ensure_sync_metadata` whenever it is
/// missing, which happens independently on every device (a folder created
/// locally, or a binding transiently dropped while the folder is absent from a
/// materialized workspace). Minting randomly made two devices bind the SAME
/// folder to DIFFERENT documents; the divergent bindings then raced in the
/// workspace-document merge, so the two devices' workspace documents never
/// settled and kept re-emitting conflicting metadata. Deriving from the folder
/// id makes every (re-)mint on every device converge on one id.
pub fn folder_document_id(folder: FolderId) -> DocumentId {
    DocumentId(stable_derived_uuid(
        FOLDER_DOCUMENT_NAMESPACE,
        folder.0.as_bytes(),
    ))
}

/// Sync metadata for a folder with the deterministic [`folder_document_id`]
/// binding. The only way folder metadata should be minted — a random binding
/// here reintroduces divergent re-minting.
pub fn folder_sync_metadata(folder: FolderId) -> SyncDocumentMeta {
    let mut meta = SyncDocumentMeta::local(SyncDocumentKind::Folder);
    meta.id = folder_document_id(folder);
    meta
}
