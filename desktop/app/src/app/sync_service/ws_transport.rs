//! Desktop `SyncTransport` over the shared WebSocket client.
//!
//! Wraps `knotq_sync::ws::WsClient` and maps its `WsRequestError` onto the exact
//! error contract the engine expects, so it is a drop-in for `SyncHttpClient`:
//!   - a server push rejection becomes `SyncPushRejected` (engine self-heals),
//!   - a not-connected/dropped/timed-out socket becomes `SyncNetworkUnreachable`
//!     (the scheduler treats it as offline AND a fallback transport can retry HTTP).
//!
//! The actual socket (tungstenite) lives behind the `ws-sync` feature in
//! `ws_socket`; this mapping layer is always compiled and unit-tested because it
//! encodes the subtle self-heal contract.
//!
//! NOTE: not yet wired into the live sync run — see WEBSOCKET_SYNC_DECISIONS.md.
#![allow(dead_code)]

use std::sync::Arc;

use anyhow::{anyhow, Result};
use knotq_sync::ws::{WsClient, WsRequestError};
use knotq_sync::{
    BatchPullRequest, BatchPullResponse, BatchPushRequest, BatchPushResponse, SyncPushRejected,
    SyncTransport,
};

use super::{SyncHttpClient, SyncNetworkUnreachable, SyncProtocolOutdated, SyncUnauthorized};

/// The live transport: prefer the WebSocket when it is connected, fall back to
/// HTTP otherwise. A WS *transport hiccup* (not connected / dropped / timed out)
/// silently falls back to HTTP for this run, so a sync always completes. A WS
/// *server rejection* is returned (mapped), NOT retried over HTTP — it would be
/// rejected the same way, and on push the engine self-heals from it.
pub(crate) struct FallbackTransport<'a> {
    ws: Option<&'a WsClient>,
    http: &'a SyncHttpClient,
}

impl<'a> FallbackTransport<'a> {
    pub(crate) fn new(ws: Option<&'a WsClient>, http: &'a SyncHttpClient) -> Self {
        Self { ws, http }
    }
}

impl SyncTransport for FallbackTransport<'_> {
    fn pull(&self, request: &BatchPullRequest) -> Result<BatchPullResponse> {
        if let Some(ws) = self.ws {
            if ws.is_connected() {
                match ws.request_pull(request) {
                    Ok(response) => return Ok(response),
                    Err(err @ WsRequestError::Server { .. }) => return Err(map_pull_error(err)),
                    Err(_) => { /* transport hiccup → fall back to HTTP this run */ }
                }
            }
        }
        self.http.pull(request)
    }

    fn push(&self, request: &BatchPushRequest) -> Result<BatchPushResponse> {
        if let Some(ws) = self.ws {
            if ws.is_connected() {
                match ws.request_push(request) {
                    Ok(response) => return Ok(response),
                    Err(err @ WsRequestError::Server { .. }) => return Err(map_push_error(err)),
                    Err(_) => { /* transport hiccup → fall back to HTTP this run */ }
                }
            }
        }
        self.http.push(request)
    }
}

/// A pure-WebSocket `SyncTransport` (no fallback). Kept for completeness/tests;
/// the live path uses [`FallbackTransport`].
pub(crate) struct WsSyncTransport {
    client: Arc<WsClient>,
}

impl WsSyncTransport {
    pub(crate) fn new(client: Arc<WsClient>) -> Self {
        Self { client }
    }
}

impl SyncTransport for WsSyncTransport {
    fn pull(&self, request: &BatchPullRequest) -> Result<BatchPullResponse> {
        self.client.request_pull(request).map_err(map_pull_error)
    }

    fn push(&self, request: &BatchPushRequest) -> Result<BatchPushResponse> {
        self.client.request_push(request).map_err(map_push_error)
    }
}

/// Transport-level failures carry `SyncNetworkUnreachable` so the scheduler backs
/// off as "offline" (and a fallback transport can try HTTP); a server error frame
/// on a pull is a plain rejection (pull has no self-heal path).
fn map_pull_error(error: WsRequestError) -> anyhow::Error {
    match error {
        WsRequestError::NotConnected | WsRequestError::Disconnected | WsRequestError::Timeout => {
            unreachable_network(error)
        }
        WsRequestError::Server { status, code } => {
            if is_protocol_outdated(status, &code) {
                return protocol_outdated(&code);
            }
            if is_unauthorized(status, &code) {
                return unauthorized(&code);
            }
            anyhow!("sync backend rejected request: {code}")
        }
        WsRequestError::Decode(msg) => anyhow!("parse websocket pull response: {msg}"),
    }
}

/// Like [`map_pull_error`], but a server rejection becomes `SyncPushRejected` so
/// `batch_push_pending`'s reseed/self-heal path fires (matching the HTTP contract).
fn map_push_error(error: WsRequestError) -> anyhow::Error {
    match error {
        WsRequestError::NotConnected | WsRequestError::Disconnected | WsRequestError::Timeout => {
            unreachable_network(error)
        }
        WsRequestError::Server { status, code } => {
            // A protocol-outdated or auth rejection must NOT become
            // SyncPushRejected: the engine's reseed self-heal is for content
            // rejections (crdt_schema_invalid), and reseeding would just be
            // rejected the same way again. The scheduler either surfaces the
            // plain error (outdated) or force-refreshes and retries (auth).
            if is_protocol_outdated(status, &code) {
                return protocol_outdated(&code);
            }
            if is_unauthorized(status, &code) {
                return unauthorized(&code);
            }
            anyhow::Error::new(SyncPushRejected { code: code.clone() })
                .context(format!("sync backend rejected request: {code}"))
        }
        WsRequestError::Decode(msg) => anyhow!("parse websocket push response: {msg}"),
    }
}

fn is_unauthorized(status: Option<u16>, code: &str) -> bool {
    status == Some(401) || code == "unauthorized"
}

/// See `http::is_protocol_outdated` — the WS backend maps the same 426 /
/// `client_protocol_outdated` rejection into an error frame, so the mapping
/// here must recognize it identically.
fn is_protocol_outdated(status: Option<u16>, code: &str) -> bool {
    status == Some(426) || code == "client_protocol_outdated"
}

fn unauthorized(code: &str) -> anyhow::Error {
    anyhow::Error::new(SyncUnauthorized).context(format!("sync backend rejected request: {code}"))
}

fn protocol_outdated(code: &str) -> anyhow::Error {
    anyhow::Error::new(SyncProtocolOutdated).context(format!("sync backend rejected request: {code}"))
}

fn unreachable_network(error: WsRequestError) -> anyhow::Error {
    anyhow::Error::new(SyncNetworkUnreachable).context(format!("websocket sync transport: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_server_error_maps_to_sync_push_rejected() {
        let err = map_push_error(WsRequestError::Server {
            status: Some(400),
            code: "crdt_schema_invalid".to_string(),
        });
        let rejected = err
            .downcast_ref::<SyncPushRejected>()
            .expect("push server error must surface as SyncPushRejected so self-heal fires");
        assert_eq!(rejected.code, "crdt_schema_invalid");
        // It must NOT also look like a network error.
        assert!(err.downcast_ref::<SyncNetworkUnreachable>().is_none());
    }

    #[test]
    fn push_disconnect_maps_to_network_unreachable() {
        for error in [
            WsRequestError::NotConnected,
            WsRequestError::Disconnected,
            WsRequestError::Timeout,
        ] {
            let err = map_push_error(error);
            assert!(
                err.downcast_ref::<SyncNetworkUnreachable>().is_some(),
                "transport failures must be offline, not a server rejection"
            );
            assert!(err.downcast_ref::<SyncPushRejected>().is_none());
        }
    }

    #[test]
    fn pull_disconnect_maps_to_network_unreachable() {
        let err = map_pull_error(WsRequestError::Disconnected);
        assert!(err.downcast_ref::<SyncNetworkUnreachable>().is_some());
    }

    #[test]
    fn pull_server_error_is_plain_not_push_rejected() {
        let err = map_pull_error(WsRequestError::Server {
            status: Some(403),
            code: "forbidden".to_string(),
        });
        assert!(err.downcast_ref::<SyncPushRejected>().is_none());
        assert!(err.downcast_ref::<SyncNetworkUnreachable>().is_none());
    }

    #[test]
    fn auth_rejection_maps_to_sync_unauthorized_on_pull_and_push() {
        // Both the explicit 401 status and a bare `unauthorized` code must be
        // recognized, on both request kinds.
        for error in [
            WsRequestError::Server {
                status: Some(401),
                code: "unauthorized".to_string(),
            },
            WsRequestError::Server {
                status: None,
                code: "unauthorized".to_string(),
            },
        ] {
            let pull = map_pull_error(error.clone());
            assert!(
                pull.downcast_ref::<SyncUnauthorized>().is_some(),
                "pull auth rejection must surface as SyncUnauthorized"
            );
            let push = map_push_error(error);
            assert!(
                push.downcast_ref::<SyncUnauthorized>().is_some(),
                "push auth rejection must surface as SyncUnauthorized"
            );
            // A stale token must NOT fire the push reseed self-heal or read as
            // offline.
            assert!(push.downcast_ref::<SyncPushRejected>().is_none());
            assert!(push.downcast_ref::<SyncNetworkUnreachable>().is_none());
        }
    }

    #[test]
    fn protocol_outdated_rejection_maps_on_pull_and_push_not_push_rejected() {
        let error = WsRequestError::Server {
            status: Some(426),
            code: "client_protocol_outdated".to_string(),
        };
        let pull = map_pull_error(error.clone());
        assert!(pull.downcast_ref::<SyncProtocolOutdated>().is_some());

        let push = map_push_error(error);
        assert!(push.downcast_ref::<SyncProtocolOutdated>().is_some());
        // Must NOT trigger the reseed self-heal — reseeding would just be
        // rejected the same way until the app is updated.
        assert!(push.downcast_ref::<SyncPushRejected>().is_none());
        assert!(push.downcast_ref::<SyncUnauthorized>().is_none());
    }
}
