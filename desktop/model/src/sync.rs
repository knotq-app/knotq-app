use serde::{Deserialize, Serialize};

use crate::{DocumentId, ShareId};

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

pub fn default_scheme_sync() -> SyncDocumentMeta {
    SyncDocumentMeta::local(SyncDocumentKind::Scheme)
}

pub fn default_folder_sync() -> SyncDocumentMeta {
    SyncDocumentMeta::local(SyncDocumentKind::Folder)
}
