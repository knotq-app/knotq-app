//! Real WebSocket transport for the integration + fuzz suites.
//!
//! This is the SAME stack the desktop runs: the shared sync engine
//! (`batch_pull_and_apply` / `batch_push_pending`) over the shared
//! [`knotq_sync::ws::WsClient`], over a real `tungstenite` socket, against the
//! real Durable Object WebSocket handler in `wrangler dev`. [`WsTransport`] is
//! the `SyncTransport` adapter (same `WsRequestError` -> `SyncPushRejected`
//! mapping the desktop adapter uses).
//!
//! Media upload/download stay on HTTP (they are not part of `SyncTransport`),
//! so a WS-backed harness keeps one `HttpClient` per device for those.

#![allow(dead_code)]

use std::io::{self, ErrorKind};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use knotq_sync::ws::{RawSocket, RawSocketFactory, WsCallbacks, WsClient, WsConfig, WsRequestError};
use knotq_sync::{
    BatchPullRequest, BatchPullResponse, BatchPushRequest, BatchPushResponse, SyncPushRejected,
    SyncTransport,
};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{ClientRequestBuilder, Message, WebSocket};

/// A real tungstenite socket behind the engine's `RawSocket` port. `ws://` only
/// (local wrangler); no TLS feature needed.
struct TgSocket {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl RawSocket for TgSocket {
    fn poll(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        if let MaybeTlsStream::Plain(tcp) = self.socket.get_mut() {
            tcp.set_read_timeout(Some(timeout))?;
        }
        match self.socket.read() {
            Ok(Message::Text(text)) => Ok(Some(text)),
            Ok(Message::Close(_)) => Err(io::Error::new(ErrorKind::ConnectionAborted, "closed")),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(err))
                if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                Ok(None)
            }
            Err(err) => Err(io::Error::other(err.to_string())),
        }
    }
    fn send(&mut self, text: &str) -> io::Result<()> {
        self.socket
            .send(Message::Text(text.to_string()))
            .map_err(|err| io::Error::other(err.to_string()))
    }
    fn close(&mut self) {
        let _ = self.socket.close(None);
    }
}

struct TgFactory {
    ws_url: String,
    token: String,
}

impl RawSocketFactory for TgFactory {
    fn connect(&self) -> io::Result<Box<dyn RawSocket>> {
        let uri = self
            .ws_url
            .parse::<tungstenite::http::Uri>()
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))?;
        let request = ClientRequestBuilder::new(uri)
            .with_header("Authorization", format!("Bearer {}", self.token));
        let (socket, _resp) =
            tungstenite::connect(request).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Box::new(TgSocket { socket }))
    }
}

/// `SyncTransport` over the shared `WsClient` — identical mapping to the desktop
/// adapter, so a scenario driven through this exercises exactly the production
/// WebSocket path.
pub struct WsTransport {
    client: Arc<WsClient>,
}

impl WsTransport {
    pub fn new(client: Arc<WsClient>) -> Self {
        Self { client }
    }
}

impl SyncTransport for WsTransport {
    fn pull(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        self.client
            .request_pull(request)
            .map_err(|error| anyhow!("ws pull: {error}"))
    }
    fn push(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        self.client.request_push(request).map_err(|error| match error {
            WsRequestError::Server { code, .. } => anyhow::Error::new(SyncPushRejected { code }),
            other => anyhow!("ws push: {other}"),
        })
    }
}

/// A transport that alternates every request between the real WebSocket and
/// plain HTTP — modelling the production `FallbackTransport` where a socket drop
/// mid-cycle sends the next request over HTTP and a reconnect brings it back.
/// Within one sync cycle the pull and the push routinely go over *different*
/// transports; that interleaving is exactly what this exercises.
pub struct MixedTransport {
    ws: WsTransport,
    http: super::http_transport::HttpClient,
    counter: std::sync::atomic::AtomicU64,
}

impl MixedTransport {
    pub fn new(client: Arc<WsClient>, http: super::http_transport::HttpClient) -> Self {
        Self {
            ws: WsTransport::new(client),
            http,
            counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn use_ws(&self) -> bool {
        self.counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            % 2
            == 0
    }
}

impl SyncTransport for MixedTransport {
    fn pull(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        if self.use_ws() {
            self.ws.pull(request)
        } else {
            self.http.pull(request)
        }
    }
    fn push(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        if self.use_ws() {
            self.ws.push(request)
        } else {
            self.http.push(request)
        }
    }
}

pub fn ws_url(base_url: &str) -> String {
    let swapped = base_url
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{swapped}/v1/sync/ws")
}

/// Connect a `WsClient` to `base_url` with `token` and block until the socket is
/// live (or panic after 10 s). `callbacks` wires `on_changed`/`on_presence`;
/// pass [`WsCallbacks::noop`] when the test does not need nudges.
pub fn connect_ws(base_url: &str, token: &str, callbacks: WsCallbacks) -> Arc<WsClient> {
    let client = Arc::new(WsClient::start(
        Box::new(TgFactory {
            ws_url: ws_url(base_url),
            token: token.to_string(),
        }),
        WsConfig::default(),
        callbacks,
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if client.is_connected() {
            return client;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("ws client never connected to backend at {base_url}");
}
