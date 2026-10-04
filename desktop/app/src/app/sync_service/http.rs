use std::io::Read;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use knotq_sync::{
    BatchPullRequest, BatchPullResponse, BatchPushRequest, BatchPushResponse, ErrorResponse,
    SquashDocumentRequest, SquashDocumentResponse, SyncPushRejected, SyncTransport,
    MAX_SYNC_MEDIA_BYTES,
};

use super::media::media_content_type;
use super::{
    SyncHttpClient, SyncMediaAsset, SyncNetworkUnreachable, SyncProtocolOutdated, SyncUnauthorized,
};

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
        let response = self
            .authorized(ureq::post(&url))
            .send_json(serde_json::to_value(request)?)
            .map_err(sync_push_http_error)?;
        read_sync_json(&url, response)
    }
}

/// Parse a sync response, and when it will not parse, SAY WHY.
///
/// `Response::into_json` consumes the body, so the bytes are gone by the time the
/// error exists: every parse failure read `parse sync response from <url>: Failed to
/// read JSON: <serde error>` and nothing else. In the field that is not enough to
/// act on — a truncated body, a proxy's HTML error page served with a 200, and a
/// field whose type the client rejects all look identical, and they need completely
/// different fixes. Reading the body first costs one buffer (and tends to be
/// *faster*: `from_reader` over a network stream reads in small chunks, while
/// `from_slice` parses one contiguous slice) and makes the difference legible.
///
/// The preview is bounded and escaped on purpose. It exists to show the SHAPE of an
/// unparseable body — `<!DOCTYPE html`, an empty body, a truncated object — and a
/// couple of hundred bytes is enough for that, while a whole sync response is the
/// user's content and does not belong in an error string.
fn read_sync_json<R: serde::de::DeserializeOwned>(
    url: &str,
    response: ureq::Response,
) -> Result<R> {
    let status = response.status();
    let content_type = response.content_type().to_string();
    let declared_length = response
        .header("content-length")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "absent".to_string());
    let content_encoding = response
        .header("content-encoding")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "identity".to_string());
    let mut body = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body)
        .with_context(|| {
            format!(
                "read sync response body from {url} (status {status}, content-type \
                 {content_type}, content-length {declared_length}, got {} byte(s) before the \
                 read failed)",
                body.len()
            )
        })?;
    serde_json::from_slice(&body).with_context(|| {
        format!(
            "parse sync response from {url}: status {status}, content-type {content_type}, \
             content-encoding {content_encoding}, content-length {declared_length}, read {} \
             byte(s), body begins {}",
            body.len(),
            body_preview(&body),
        )
    })
}

/// A bounded, escaped look at a body that would not parse.
fn body_preview(body: &[u8]) -> String {
    const PREVIEW_BYTES: usize = 200;
    if body.is_empty() {
        return "<empty>".to_string();
    }
    let shown = &body[..body.len().min(PREVIEW_BYTES)];
    let text: String = String::from_utf8_lossy(shown)
        .chars()
        .map(|c| if c.is_control() { '·' } else { c })
        .collect();
    if body.len() > PREVIEW_BYTES {
        format!("{text:?}… (truncated for this message)")
    } else {
        format!("{text:?}")
    }
}

impl super::SyncSideChannel for SyncHttpClient {
    /// `POST /v1/sync/squash` — propose a history squash. Every rejection
    /// (conflict, too-soon, content mismatch) is an expected, benign outcome
    /// the caller merely logs.
    fn squash(&self, request: &SquashDocumentRequest) -> Result<SquashDocumentResponse> {
        let url = format!("{}/v1/sync/squash", self.api_base);
        self.post_json(&url, request)
    }

    fn upload_media_asset(&self, media: SyncMediaAsset, bytes: &[u8]) -> Result<()> {
        let url = self.media_url(media);
        self.authorized(ureq::put(&url))
            .set("content-type", media_content_type(media.format))
            .send_bytes(bytes)
            .map_err(sync_http_error)?;
        Ok(())
    }

    fn download_media_asset(&self, media: SyncMediaAsset) -> Result<Option<Vec<u8>>> {
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
}

impl SyncHttpClient {
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
        let response = self
            .authorized(ureq::post(url))
            .send_json(serde_json::to_value(body)?)
            .map_err(sync_http_error)?;
        read_sync_json(url, response)
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
            if is_protocol_outdated(status, &code) {
                return protocol_outdated(&code);
            }
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
            if is_protocol_outdated(status, &code) {
                return protocol_outdated(&code);
            }
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

/// 426 (Upgrade Required) is the backend's dedicated status for a client below
/// its protocol floor — distinct from the 400-499 range `sync_push_http_error`
/// otherwise treats as a self-healing content rejection (reseed-and-retry would
/// just be rejected again until the app updates).
fn is_protocol_outdated(status: u16, code: &str) -> bool {
    status == 426 || code == "client_protocol_outdated"
}

fn protocol_outdated(code: &str) -> anyhow::Error {
    anyhow::Error::new(SyncProtocolOutdated)
        .context(format!("sync backend rejected request: {code}"))
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

    #[test]
    fn status_426_maps_to_sync_protocol_outdated() {
        let err = sync_http_error(status_error(426, &error_body("client_protocol_outdated")));
        assert!(err.downcast_ref::<SyncProtocolOutdated>().is_some());
        assert!(err.downcast_ref::<SyncUnauthorized>().is_none());
    }

    #[test]
    fn push_426_is_protocol_outdated_not_push_rejected() {
        // Must NOT become SyncPushRejected: reseeding and re-pushing would just
        // be rejected the same way until the app is updated.
        let err = sync_push_http_error(status_error(426, &error_body("client_protocol_outdated")));
        assert!(err.downcast_ref::<SyncProtocolOutdated>().is_some());
        assert!(err.downcast_ref::<SyncPushRejected>().is_none());
        assert!(err.downcast_ref::<SyncUnauthorized>().is_none());
    }

    /// A body that will not parse must say what came back, because the three
    /// causes need completely different fixes and the old message could not tell
    /// them apart: a proxy's HTML page served with a 200, a truncated body, and a
    /// field whose type the client rejects all read as `Failed to read JSON`.
    ///
    /// Nothing tested this path at all before — `TestServer` is in-memory and never
    /// serializes, and the wrangler suite only ever sees a healthy server — which is
    /// why a real parse failure in the field came with nothing to act on.
    #[test]
    fn an_unparseable_response_says_what_came_back() {
        let html = "<!DOCTYPE html><html><head><title>502 Bad Gateway</title></head>";
        let response = ureq::Response::new(200, "OK", html).unwrap();
        let err = read_sync_json::<serde_json::Value>("https://api.example/v1/sync/pull", response)
            .expect_err("HTML is not a sync response");
        let report = format!("{err:#}");

        assert!(report.contains("status 200"), "no status: {report}");
        assert!(
            report.contains("read 64 byte(s)"),
            "no body length: {report}"
        );
        assert!(
            report.contains("<!DOCTYPE html"),
            "no body preview, so the shape of the body is still invisible: {report}"
        );
    }

    /// An empty 200 is its own diagnosis — "the server said OK and sent nothing" —
    /// and must not be reported as though some content failed to parse.
    #[test]
    fn an_empty_response_is_named_as_empty() {
        let response = ureq::Response::new(200, "OK", "").unwrap();
        let err = read_sync_json::<serde_json::Value>("https://api.example/v1/sync/pull", response)
            .expect_err("an empty body is not a sync response");
        let report = format!("{err:#}");
        assert!(report.contains("read 0 byte(s)"), "{report}");
        assert!(report.contains("<empty>"), "{report}");
    }

    /// The preview is bounded: a whole sync response is the user's content and does
    /// not belong in an error string, and a couple of hundred bytes is enough to see
    /// the shape.
    #[test]
    fn the_body_preview_is_bounded() {
        let long = format!("{{\"documents\": \"{}\"", "x".repeat(10_000));
        let response = ureq::Response::new(200, "OK", &long).unwrap();
        let err = read_sync_json::<serde_json::Value>("https://api.example/v1/sync/pull", response)
            .expect_err("truncated JSON is not a sync response");
        let report = format!("{err:#}");
        assert!(report.contains("truncated for this message"), "{report}");
        assert!(
            report.len() < 1_000,
            "the error carried {} characters of the body",
            report.len()
        );
    }
}
