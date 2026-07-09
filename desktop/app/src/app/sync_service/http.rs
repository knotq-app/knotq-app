use std::io::Read;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use knotq_sync::{
    BatchPullRequest, BatchPullResponse, BatchPushRequest, BatchPushResponse, ErrorResponse,
    SquashDocumentRequest, SquashDocumentResponse, SyncPushRejected, SyncTransport,
    MAX_SYNC_MEDIA_BYTES,
};

use super::media::media_content_type;
use super::{SyncHttpClient, SyncMediaAsset, SyncNetworkUnreachable, SyncUnauthorized};

impl SyncTransport for SyncHttpClient {
    fn pull(&self, request: &BatchPullRequest) -> Result<BatchPullResponse> {
        let url = format!("{}/v1/sync/pull", self.api_base);
        self.post_json(&url, request)
    }

    fn push(&self, request: &BatchPushRequest) -> Result<BatchPushResponse> {
        let url = format!("{}/v1/sync/push", self.api_base);
        // Push-specific error mapping: a deterministic 4xx rejection must carry
        // the typed `SyncPushRejected` so the engine's self-heal (reseed) and the
        // scheduler's epoch-stale re-pull can react — mirroring the WebSocket
        // transport's `map_push_error`. The plain string this used to return
        // silently disabled both on the HTTP fallback path.
        self.authorized(ureq::post(&url))
            .send_json(serde_json::to_value(request)?)
            .map_err(sync_push_http_error)?
            .into_json()
            .with_context(|| format!("parse sync response from {url}"))
    }
}

impl SyncHttpClient {
    /// `POST /v1/sync/squash` — propose a history squash. Every rejection
    /// (conflict, too-soon, content mismatch) is an expected, benign outcome
    /// the caller merely logs.
    pub(super) fn squash(&self, request: &SquashDocumentRequest) -> Result<SquashDocumentResponse> {
        let url = format!("{}/v1/sync/squash", self.api_base);
        self.post_json(&url, request)
    }

    pub(super) fn upload_media_asset(&self, media: SyncMediaAsset, bytes: &[u8]) -> Result<()> {
        let url = self.media_url(media);
        self.authorized(ureq::put(&url))
            .set("content-type", media_content_type(media.format))
            .send_bytes(bytes)
            .map_err(sync_http_error)?;
        Ok(())
    }

    pub(super) fn download_media_asset(&self, media: SyncMediaAsset) -> Result<Option<Vec<u8>>> {
        let url = self.media_url(media);
        let response = match self.authorized(ureq::get(&url)).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(404, response)) => {
                let code = response
                    .into_json::<ErrorResponse>()
                    .map(|error| error.code)
                    .unwrap_or_else(|_| "404".to_string());
                if code == "not_found" {
                    return Ok(None);
                }
                return Err(anyhow!("sync backend rejected request: {code}"));
            }
            Err(error) => return Err(sync_http_error(error)),
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
                "sync backend returned image {} above the {} byte sync limit",
                media.image_name(),
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        Ok(Some(bytes))
    }

    fn media_url(&self, media: SyncMediaAsset) -> String {
        format!(
            "{}/v1/sync/documents/{}/media/{}",
            self.api_base,
            media.document,
            media.image_name()
        )
    }

    fn post_json<T, R>(&self, url: &str, body: &T) -> Result<R>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.authorized(ureq::post(url))
            .send_json(serde_json::to_value(body)?)
            .map_err(sync_http_error)?
            .into_json()
            .with_context(|| format!("parse sync response from {url}"))
    }

    fn authorized(&self, request: ureq::Request) -> ureq::Request {
        // Individual HTTP requests are given 30 s to complete regardless of the
        // current poll cadence.
        const HTTP_TIMEOUT: StdDuration = StdDuration::from_secs(30);
        request
            .timeout(HTTP_TIMEOUT)
            .set("authorization", &format!("Bearer {}", self.bearer_token))
    }
}

// Push-path variant of `sync_http_error`: auth and transport failures map the
// same way, but any other 4xx becomes the typed `SyncPushRejected` (its Display
// already reads "sync backend rejected request: <code>", so no extra context —
// that would print the message twice, like the WS path's doubled form). A 5xx
// stays an untyped transient error so the engine does NOT reseed on it.
fn sync_push_http_error(error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(status, response) => {
            let code = response
                .into_json::<ErrorResponse>()
                .map(|error| error.code)
                .unwrap_or_else(|_| status.to_string());
            if status == 401 || code == "unauthorized" {
                return anyhow::Error::new(SyncUnauthorized)
                    .context(format!("sync backend rejected request: {code}"));
            }
            if (400..500).contains(&status) {
                return anyhow::Error::new(SyncPushRejected { code });
            }
            anyhow!("sync backend rejected request: {code}")
        }
        error => anyhow::Error::new(SyncNetworkUnreachable)
            .context(format!("sync backend request failed: {error}")),
    }
}

fn sync_http_error(error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(status, response) => {
            let code = response
                .into_json::<ErrorResponse>()
                .map(|error| error.code)
                .unwrap_or_else(|_| status.to_string());
            // Attach SyncUnauthorized so the scheduler can force-refresh the
            // token and retry instead of surfacing an opaque failure.
            if status == 401 || code == "unauthorized" {
                return anyhow::Error::new(SyncUnauthorized)
                    .context(format!("sync backend rejected request: {code}"));
            }
            anyhow!("sync backend rejected request: {code}")
        }
        // Transport / connection failures: attach SyncNetworkUnreachable so the
        // scheduler can detect "offline" via downcast_ref.
        error => anyhow::Error::new(SyncNetworkUnreachable)
            .context(format!("sync backend request failed: {error}")),
    }
}

pub(super) fn normalize_api_base(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!("sync API URL is empty"));
    }
    // The bearer token and all workspace contents travel over this URL. Refuse
    // plaintext HTTP to anything other than a loopback dev server so a misconfig
    // (or tampered settings file) can't silently leak credentials in the clear.
    if !is_secure_api_base(trimmed) {
        return Err(anyhow!("sync API URL must use https:// (got {trimmed})"));
    }
    Ok(trimmed.to_string())
}

fn is_secure_api_base(url: &str) -> bool {
    if let Some(host) = url.strip_prefix("https://") {
        return !host.is_empty();
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest
            .split(['/', ':'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        return matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_error(status: u16, body: &str) -> ureq::Error {
        let response = ureq::Response::new(status, "status", body).expect("synthetic response");
        ureq::Error::Status(status, response)
    }

    /// The backend's error body shape (`ErrorResponse` requires BOTH fields —
    /// a body missing `message` falls back to the bare status code).
    fn error_body(code: &str) -> String {
        format!(r#"{{"code":"{code}","message":"test"}}"#)
    }

    #[test]
    fn status_401_maps_to_sync_unauthorized() {
        let err = sync_http_error(status_error(401, &error_body("unauthorized")));
        assert!(
            err.downcast_ref::<SyncUnauthorized>().is_some(),
            "401 must surface as SyncUnauthorized so the scheduler force-refreshes and retries"
        );
        assert!(err.downcast_ref::<SyncNetworkUnreachable>().is_none());
        assert!(format!("{err:#}").contains("unauthorized"));
    }

    #[test]
    fn status_401_with_unparseable_body_still_maps_to_sync_unauthorized() {
        // A proxy or edge error page can replace the JSON body; the bare status
        // must still be recognized as an auth rejection.
        let err = sync_http_error(status_error(401, "<html>gateway says no</html>"));
        assert!(err.downcast_ref::<SyncUnauthorized>().is_some());
    }

    #[test]
    fn unauthorized_code_maps_to_sync_unauthorized_regardless_of_status() {
        let err = sync_http_error(status_error(403, &error_body("unauthorized")));
        assert!(err.downcast_ref::<SyncUnauthorized>().is_some());
    }

    #[test]
    fn push_4xx_rejection_is_typed_sync_push_rejected() {
        // The engine's reseed self-heal and the epoch-stale retry both downcast
        // for SyncPushRejected; an untyped string silently disables them.
        let err = sync_push_http_error(status_error(409, &error_body("document_epoch_stale")));
        let rejected = err
            .downcast_ref::<SyncPushRejected>()
            .expect("push 4xx must be SyncPushRejected");
        assert_eq!(rejected.code, "document_epoch_stale");
        assert!(format!("{err:#}").contains("document_epoch_stale"));
    }

    #[test]
    fn push_401_is_unauthorized_not_rejected() {
        let err = sync_push_http_error(status_error(401, &error_body("unauthorized")));
        assert!(err.downcast_ref::<SyncUnauthorized>().is_some());
        assert!(err.downcast_ref::<SyncPushRejected>().is_none());
    }

    #[test]
    fn push_5xx_stays_untyped_transient() {
        // A server-side failure must NOT trigger the engine's reseed (it would
        // re-queue full snapshots on every outage).
        let err = sync_push_http_error(status_error(500, &error_body("internal_error")));
        assert!(err.downcast_ref::<SyncPushRejected>().is_none());
        assert!(err.downcast_ref::<SyncUnauthorized>().is_none());
    }

    #[test]
    fn content_rejection_is_not_unauthorized() {
        // crdt_schema_invalid must keep flowing to the engine's reseed self-heal,
        // never into the token-refresh retry loop.
        let err = sync_http_error(status_error(400, &error_body("crdt_schema_invalid")));
        assert!(err.downcast_ref::<SyncUnauthorized>().is_none());
        assert!(err.downcast_ref::<SyncNetworkUnreachable>().is_none());
        assert!(format!("{err:#}").contains("crdt_schema_invalid"));
    }
}
