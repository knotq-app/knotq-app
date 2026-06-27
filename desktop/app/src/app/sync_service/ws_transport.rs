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

use super::SyncNetworkUnreachable;

/// A `SyncTransport` that carries batched pull/push over a persistent WebSocket.
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
        WsRequestError::NotConnected
        | WsRequestError::Disconnected
        | WsRequestError::Timeout => unreachable_network(error),
        WsRequestError::Server { code, .. } => {
            anyhow!("sync backend rejected request: {code}")
        }
        WsRequestError::Decode(msg) => anyhow!("parse websocket pull response: {msg}"),
    }
}

/// Like [`map_pull_error`], but a server rejection becomes `SyncPushRejected` so
/// `batch_push_pending`'s reseed/self-heal path fires (matching the HTTP contract).
fn map_push_error(error: WsRequestError) -> anyhow::Error {
    match error {
        WsRequestError::NotConnected
        | WsRequestError::Disconnected
        | WsRequestError::Timeout => unreachable_network(error),
        WsRequestError::Server { code, .. } => anyhow::Error::new(SyncPushRejected { code: code.clone() })
            .context(format!("sync backend rejected request: {code}")),
        WsRequestError::Decode(msg) => anyhow!("parse websocket push response: {msg}"),
    }
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
}
