//! End-to-end WebSocket sync tests against the REAL Cloudflare Worker running in
//! local dev mode (`wrangler dev`). These drive the FULL stack: the shared sync
//! engine (`batch_pull_and_apply`/`batch_push_pending`) over the shared
//! `knotq_sync::ws::WsClient`, over a real `tungstenite` socket, against the real
//! Durable Object WebSocket handler.
//!
//! ## How to run
//!
//! ```sh
//! # Terminal 1 — backend in test mode
//! cd app/backend/cloudflare
//! pnpm wrangler dev --local --port 8788 --var KNOTQ_TEST_MODE:1 \
//!   --persist-to .wrangler/integration-test-state
//!
//! # Terminal 2
//! export KNOTQ_SYNC_BACKEND_URL=http://127.0.0.1:8788
//! cargo test -p knotq-sync --test ws_integration -- --nocapture
//! ```
//!
//! Skips (does not fail) when `KNOTQ_SYNC_BACKEND_URL` is unset.

mod common;

use std::env;
use std::io::{self, ErrorKind};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use common::http_transport::{backend_bootstrap, unique_test_email};
use common::TestDevice;
use knotq_model::{Workspace, WorkspaceId};
use knotq_sync::ws::{
    RawSocket, RawSocketFactory, WsCallbacks, WsClient, WsConfig, WsRequestError,
};
use knotq_sync::{
    BatchPullRequest, BatchPullResponse, BatchPushRequest, BatchPushResponse, SyncPushRejected,
    SyncTransport,
};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{ClientRequestBuilder, Message, WebSocket};

fn backend_url() -> Option<String> {
    match env::var("KNOTQ_SYNC_BACKEND_URL") {
        Ok(url) if !url.is_empty() => Some(url.trim_end_matches('/').to_string()),
        _ => {
            println!("[ws_integration] KNOTQ_SYNC_BACKEND_URL not set — skipping.");
            None
        }
    }
}

fn make_device(workspace_id: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(workspace_id);
    base.ensure_sync_metadata();
    TestDevice::new_from_base(&base, workspace_id)
}

// ── A real tungstenite RawSocket (ws://, no TLS feature needed locally) ──────

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
            Err(err) => Err(io::Error::new(ErrorKind::Other, err.to_string())),
        }
    }
    fn send(&mut self, text: &str) -> io::Result<()> {
        self.socket
            .send(Message::Text(text.to_string()))
            .map_err(|err| io::Error::new(ErrorKind::Other, err.to_string()))
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
        let request =
            ClientRequestBuilder::new(uri).with_header("Authorization", format!("Bearer {}", self.token));
        let (socket, _resp) = tungstenite::connect(request)
            .map_err(|e| io::Error::new(ErrorKind::Other, e.to_string()))?;
        Ok(Box::new(TgSocket { socket }))
    }
}

/// SyncTransport over the shared WsClient (same mapping the desktop adapter uses).
struct WsTransport {
    client: Arc<WsClient>,
}

impl SyncTransport for WsTransport {
    fn pull(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        self.client.request_pull(request).map_err(map_pull)
    }
    fn push(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        self.client.request_push(request).map_err(map_push)
    }
}

fn map_pull(error: WsRequestError) -> anyhow::Error {
    anyhow!("ws pull: {error}")
}

fn map_push(error: WsRequestError) -> anyhow::Error {
    match error {
        WsRequestError::Server { code, .. } => anyhow::Error::new(SyncPushRejected { code }),
        other => anyhow!("ws push: {other}"),
    }
}

fn ws_url(base_url: &str) -> String {
    let swapped = base_url
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{swapped}/v1/sync/ws")
}

fn start_client(base_url: &str, token: &str, callbacks: WsCallbacks) -> Arc<WsClient> {
    Arc::new(WsClient::start(
        Box::new(TgFactory {
            ws_url: ws_url(base_url),
            token: token.to_string(),
        }),
        WsConfig::default(),
        callbacks,
    ))
}

fn wait_connected(client: &WsClient) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if client.is_connected() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("ws client never connected to backend");
}

#[test]
fn ws_two_device_convergence_over_real_socket() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let email = unique_test_email("ws-converge");
    let resp_a = backend_bootstrap(&base_url, &email).expect("bootstrap A");
    let resp_b = backend_bootstrap(&base_url, &email).expect("bootstrap B");
    let workspace_id: WorkspaceId = resp_a.workspace_id.parse().expect("uuid");

    let mut device_a = make_device(workspace_id);
    let mut device_b = make_device(workspace_id);

    let client_a = start_client(&base_url, &resp_a.bearer_token, WsCallbacks::noop());
    let client_b = start_client(&base_url, &resp_b.bearer_token, WsCallbacks::noop());
    wait_connected(&client_a);
    wait_connected(&client_b);
    let ws_a = WsTransport {
        client: Arc::clone(&client_a),
    };
    let ws_b = WsTransport {
        client: Arc::clone(&client_b),
    };

    // A creates a scheme and pushes it over the WebSocket.
    let scheme = device_a.add_scheme("WS Plan", &["alpha", "beta"]);
    device_a.try_sync_with(&ws_a).expect("A push over ws");

    // B pulls over the WebSocket and discovers it.
    device_b.try_sync_with(&ws_b).expect("B pull over ws");
    assert!(
        device_b.workspace.schemes.contains_key(&scheme),
        "device B must discover the scheme pushed over the websocket"
    );
    let items_b: Vec<String> = device_b.workspace.schemes[&scheme]
        .items
        .iter()
        .map(|i| i.text())
        .collect();
    assert!(
        items_b.iter().any(|t| t == "alpha") && items_b.iter().any(|t| t == "beta"),
        "device B must see both items over ws; got {items_b:?}"
    );

    // B edits and pushes; A pulls and converges — all over the socket.
    device_b.append_line(scheme, "gamma");
    device_b.try_sync_with(&ws_b).expect("B push over ws");
    device_a.try_sync_with(&ws_a).expect("A pull over ws");
    let items_a: Vec<String> = device_a.workspace.schemes[&scheme]
        .items
        .iter()
        .map(|i| i.text())
        .collect();
    assert!(
        items_a.iter().any(|t| t == "gamma"),
        "device A must converge to gamma over ws; got {items_a:?}"
    );
}

#[test]
fn ws_push_broadcasts_changed_to_other_socket() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let email = unique_test_email("ws-changed");
    let resp_a = backend_bootstrap(&base_url, &email).expect("bootstrap A");
    let resp_b = backend_bootstrap(&base_url, &email).expect("bootstrap B");
    let workspace_id: WorkspaceId = resp_a.workspace_id.parse().expect("uuid");

    let mut device_a = make_device(workspace_id);

    let changed = Arc::new(AtomicUsize::new(0));
    let client_a = start_client(&base_url, &resp_a.bearer_token, WsCallbacks::noop());
    let client_b = start_client(
        &base_url,
        &resp_b.bearer_token,
        WsCallbacks {
            on_changed: {
                let changed = Arc::clone(&changed);
                Box::new(move || {
                    changed.fetch_add(1, Ordering::SeqCst);
                })
            },
            on_presence: Box::new(|_| {}),
        },
    );
    wait_connected(&client_a);
    wait_connected(&client_b);
    // B must have issued at least one request so the DO learns its replica before
    // we assert it is nudged — do an initial pull.
    let ws_b = WsTransport {
        client: Arc::clone(&client_b),
    };
    let mut device_b = make_device(workspace_id);
    device_b.try_sync_with(&ws_b).expect("B initial pull");

    // A pushes a change; the DO should broadcast `changed` to B's socket.
    let ws_a = WsTransport {
        client: Arc::clone(&client_a),
    };
    device_a.add_scheme("Broadcast", &["x"]);
    device_a.try_sync_with(&ws_a).expect("A push");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && changed.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        changed.load(Ordering::SeqCst) >= 1,
        "device B's socket should receive a `changed` nudge after A pushes"
    );
}
