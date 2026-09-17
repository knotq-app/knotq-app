//! One simulated account: an in-memory backend and its canonical workspace id.

use chrono::{Duration, Utc};
use knotq_model::{SyncAccountSettings, WorkspaceId};
use knotq_sync::testing::MemoryServer;
use knotq_sync::{SquashDocumentRequest, SquashDocumentResponse};

use super::super::{SyncMediaAsset, SyncSideChannel};

pub(super) struct Account {
    pub(super) index: usize,
    pub(super) workspace: WorkspaceId,
    pub(super) server: MemoryServer,
}

impl Account {
    pub(super) fn new(index: usize) -> Self {
        Self {
            index,
            workspace: WorkspaceId::new(),
            server: MemoryServer::default(),
        }
    }

    /// The signed-in account settings a device carries — what sign-in stores.
    pub(super) fn settings(&self) -> SyncAccountSettings {
        SyncAccountSettings {
            api_base: format!("https://account-{}.fuzz.invalid", self.index),
            user_id: format!("user-{}", self.index),
            session_id: None,
            workspace_id: Some(self.workspace.to_string()),
            email: format!("user-{}@example.com", self.index),
            supports_sync: true,
            bearer_token: "fuzz".to_string(),
            expires_at: Utc::now() + Duration::days(365),
            refresh_token: None,
            refresh_expires_at: None,
            account_status: None,
        }
    }
}

/// The in-memory backend serves media and squash the way the HTTP client does.
impl SyncSideChannel for MemoryServer {
    fn upload_media_asset(&self, media: SyncMediaAsset, bytes: &[u8]) -> anyhow::Result<()> {
        self.upload_media(media.document, &media.image_name(), bytes.to_vec())
    }

    fn download_media_asset(&self, media: SyncMediaAsset) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self.download_media(media.document, &media.image_name()))
    }

    fn squash(&self, request: &SquashDocumentRequest) -> anyhow::Result<SquashDocumentResponse> {
        let bytes_after = request.state_v1.len() as u64;
        let (seq, epoch) = self.try_squash_document(
            request.document,
            request.base_seq,
            request.base_epoch,
            request.state_v1.clone(),
        )?;
        Ok(SquashDocumentResponse {
            document: request.document,
            epoch,
            seq,
            bytes_before: 0,
            bytes_after,
        })
    }
}
