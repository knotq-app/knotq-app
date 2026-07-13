//! HTTP transport for the backend integration tests.
//!
//! [`HttpTransport`] implements the real [`SyncTransport`] trait against the
//! live (or local-dev) Cloudflare Worker backend, serialising/deserialising the
//! same JSON wire format the desktop `SyncHttpClient` uses so the integration
//! suite exercises exactly the production code path end-to-end.
//!
//! Media upload/download are provided as freestanding methods on [`HttpClient`]
//! (mirrors the desktop `SyncHttpClient` and the in-process `TestServer` media
//! helpers) because media is not part of the `SyncTransport` trait.

#![allow(dead_code)]

use std::io::Read;

use anyhow::{anyhow, Context, Result};
use knotq_model::{DocumentId, SyncDocumentKind};
use knotq_sync::{
    BatchPullRequest, BatchPullResponse, BatchPushRequest, BatchPushResponse, ErrorResponse,
    SyncPushRejected, SyncTransport, MAX_SYNC_MEDIA_BYTES,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// The response shape from `POST /__test/bootstrap`.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct BootstrapResponse {
    pub user_id: String,
    pub workspace_id: String,
    pub bearer_token: String,
    pub expires_at: String,
}

/// Bootstrap a test account on the backend. `base_url` is the wrangler dev
/// URL (e.g. `http://127.0.0.1:8788`). `email` must be unique per test so runs
/// are isolated and rerunnable.
pub fn backend_bootstrap(base_url: &str, email: &str) -> Result<BootstrapResponse> {
    let url = format!("{base_url}/__test/bootstrap");
    let body = serde_json::json!({ "email": email });
    let response = ureq::post(&url)
        .set("content-type", "application/json")
        .send_json(body)
        .with_context(|| format!("POST {url}"))?;
    response
        .into_json::<BootstrapResponse>()
        .with_context(|| "parse bootstrap response")
}

/// A lightweight HTTP client that holds a bearer token and base URL.
/// - `SyncTransport` is implemented directly on this type for pull/push.
/// - `upload_media` / `download_media` are extra methods for test scenarios.
#[derive(Clone, Debug)]
pub struct HttpClient {
    pub api_base: String,
    pub bearer_token: String,
}

impl HttpClient {
    /// Construct from a bootstrap response.
    pub fn from_bootstrap(base_url: &str, bootstrap: &BootstrapResponse) -> Self {
        Self {
            api_base: base_url.trim_end_matches('/').to_string(),
            bearer_token: bootstrap.bearer_token.clone(),
        }
    }

    /// Upload raw bytes for a media asset. Mirrors the desktop's
    /// `SyncHttpClient::upload_media_asset` / `PUT /v1/sync/documents/{document}/media/{name}`.
    pub fn upload_media(&self, document: DocumentId, image_name: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "media asset {} exceeds the {} byte limit ({} bytes)",
                image_name,
                MAX_SYNC_MEDIA_BYTES,
                bytes.len(),
            ));
        }
        let url = self.media_url(document, image_name);
        self.authorized(ureq::put(&url))
            .set("content-type", "application/octet-stream")
            .send_bytes(bytes)
            .map_err(|e| http_error(e, &url))?;
        Ok(())
    }

    /// Download a media asset. Returns `None` on 404.
    /// Mirrors `GET /v1/sync/documents/{document}/media/{name}`.
    pub fn download_media(
        &self,
        document: DocumentId,
        image_name: &str,
    ) -> Result<Option<Vec<u8>>> {
        let url = self.media_url(document, image_name);
        let response = match self.authorized(ureq::get(&url)).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(404, r)) => {
                let code = r
                    .into_json::<ErrorResponse>()
                    .map(|e| e.code)
                    .unwrap_or_else(|_| "404".to_string());
                if code == "not_found" {
                    return Ok(None);
                }
                return Err(anyhow!("sync backend rejected request: {code}"));
            }
            Err(error) => return Err(http_error(error, &url)),
        };
        let mut reader = response
            .into_reader()
            .take((MAX_SYNC_MEDIA_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .with_context(|| format!("read media response from {url}"))?;
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "sync backend returned image above the {} byte sync limit",
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        Ok(Some(bytes))
    }

    // ---- internal helpers ---------------------------------------------------

    fn media_url(&self, document: DocumentId, image_name: &str) -> String {
        format!(
            "{}/v1/sync/documents/{}/media/{}",
            self.api_base, document, image_name
        )
    }

    fn post_json<T: Serialize, R: DeserializeOwned>(&self, url: &str, body: &T) -> Result<R> {
        self.authorized(ureq::post(url))
            .set("content-type", "application/json")
            .send_json(serde_json::to_value(body).with_context(|| "serialise request")?)
            .map_err(|e| http_error(e, url))?
            .into_json::<R>()
            .with_context(|| format!("parse sync response from {url}"))
    }

    fn authorized(&self, request: ureq::Request) -> ureq::Request {
        use std::time::Duration;
        request
            .timeout(Duration::from_secs(30))
            .set("authorization", &format!("Bearer {}", self.bearer_token))
    }
}

impl SyncTransport for HttpClient {
    fn pull(&self, request: &BatchPullRequest) -> Result<BatchPullResponse> {
        let url = format!("{}/v1/sync/pull", self.api_base);
        self.post_json(&url, request)
    }

    fn push(&self, request: &BatchPushRequest) -> Result<BatchPushResponse> {
        let url = format!("{}/v1/sync/push", self.api_base);
        self.post_json(&url, request)
    }
}

/// Map a `ureq` error to an `anyhow::Error` that carries `SyncPushRejected` for
/// 4xx server responses (so `batch_push_pending`'s self-heal path fires), or a
/// plain context string for transport failures.
fn http_error(error: ureq::Error, url: &str) -> anyhow::Error {
    match error {
        ureq::Error::Status(_, response) => {
            // Try to read the ErrorResponse body; fall back to a generic code.
            let code = response
                .into_json::<ErrorResponse>()
                .map(|e| e.code)
                .unwrap_or_else(|_| "unknown_error".to_string());
            anyhow::Error::new(SyncPushRejected { code: code.clone() })
                .context(format!("sync backend rejected request: {code}"))
        }
        transport_err => anyhow!("sync backend HTTP request to {url} failed: {transport_err}"),
    }
}

/// Helper: build a unique email for a test using a UUID so parallel test runs
/// don't collide and each test gets a fresh isolated backend workspace.
pub fn unique_test_email(label: &str) -> String {
    format!("integration-{label}-{}@test.knotq", uuid::Uuid::new_v4())
}

/// Helper: build a scheme document push payload for a raw orphan-injection test.
/// Constructs the `BatchPushRequest` with `Vec<PushDocumentUpdates>` carrying a
/// single document's Yjs update bytes, bypassing `TestDevice`.
pub fn orphan_push_request(
    document: DocumentId,
    kind: SyncDocumentKind,
    update_bytes: Vec<u8>,
) -> BatchPushRequest {
    use chrono::Utc;
    use knotq_model::ReplicaId;
    use knotq_sync::{NotificationScheduleSnapshot, PushDocumentUpdates};
    BatchPushRequest {
        replica_id: ReplicaId::new(),
        documents: vec![PushDocumentUpdates {
            document,
            kind,
            epoch: 0,
            updates: vec![update_bytes],
        }],
        notification_schedule_changed: false,
        client_protocol_version: knotq_sync::CLIENT_SYNC_PROTOCOL_VERSION,
        notification_schedule: Some(NotificationScheduleSnapshot {
            sequence: 0,
            // The real backend requires a 64-char sha256 hex hash and a
            // non-empty window (window_end > window_start).
            hash: "0".repeat(64),
            window_start: Utc::now(),
            window_end: Utc::now() + chrono::Duration::hours(1),
            occurrence_count: 0,
        }),
    }
}
