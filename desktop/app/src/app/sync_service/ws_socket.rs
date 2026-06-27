//! Desktop tungstenite-backed `RawSocket` for `knotq_sync::ws`.
//!
//! Behind the `ws-sync` feature so the default build pulls no extra TLS stack
//! while the transport is being brought up. Uses a blocking tungstenite socket
//! with a per-poll read timeout to interleave reads and outbound sends on the
//! single supervisor thread (the same blocking style as the `ureq` HTTP path).
//!
//! NOTE: not yet wired into the live sync run — see WEBSOCKET_SYNC_DECISIONS.md.
#![allow(dead_code)]
use std::io::{self, ErrorKind};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use knotq_sync::ws::{
    RawSocket, RawSocketFactory, WsCallbacks, WsClient, WsConfig,
};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{ClientRequestBuilder, Message, WebSocket};

/// A token provider, called on every (re)connect so a fresh, unexpired bearer
/// token is used after a refresh.
pub(crate) type TokenProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

struct TungsteniteSocket {
    socket: Socket,
}

impl RawSocket for TungsteniteSocket {
    fn poll(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        set_read_timeout(self.socket.get_mut(), Some(timeout))?;
        match self.socket.read() {
            Ok(Message::Text(text)) => Ok(Some(text)),
            Ok(Message::Close(_)) => {
                Err(io::Error::new(ErrorKind::ConnectionAborted, "ws closed by server"))
            }
            // Ping/Pong are answered by tungstenite internally; ignore other frames.
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

fn set_read_timeout(
    stream: &mut MaybeTlsStream<TcpStream>,
    timeout: Option<Duration>,
) -> io::Result<()> {
    match stream {
        MaybeTlsStream::Plain(tcp) => tcp.set_read_timeout(timeout),
        MaybeTlsStream::Rustls(tls) => tls.sock.set_read_timeout(timeout),
        // MaybeTlsStream is non_exhaustive; only Plain/Rustls are reachable with our
        // feature set. Anything else: leave blocking (best effort).
        _ => Ok(()),
    }
}

struct TungsteniteFactory {
    ws_url: String,
    token_provider: TokenProvider,
}

impl RawSocketFactory for TungsteniteFactory {
    fn connect(&self) -> io::Result<Box<dyn RawSocket>> {
        let token = (self.token_provider)()
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "no auth token for ws connect"))?;
        let uri = self
            .ws_url
            .parse::<tungstenite::http::Uri>()
            .map_err(|err| io::Error::new(ErrorKind::InvalidInput, err.to_string()))?;
        // ClientRequestBuilder adds the WebSocket handshake headers; we add auth.
        let request =
            ClientRequestBuilder::new(uri).with_header("Authorization", format!("Bearer {token}"));
        let (socket, _response) = tungstenite::connect(request)
            .map_err(|err| io::Error::new(ErrorKind::Other, err.to_string()))?;
        Ok(Box::new(TungsteniteSocket { socket }))
    }
}

/// Turn an `https://host[/...]` API base into the `wss://host/v1/sync/ws` endpoint.
pub(crate) fn ws_url_from_api_base(api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    let swapped = if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        trimmed.to_string()
    };
    format!("{swapped}/v1/sync/ws")
}

/// Start a persistent WebSocket sync client against `api_base`. The supervisor
/// reconnects with backoff for the client's lifetime, re-reading the token from
/// `token_provider` on each connect.
pub(crate) fn connect_workspace_ws(
    api_base: &str,
    token_provider: TokenProvider,
    config: WsConfig,
    callbacks: WsCallbacks,
) -> WsClient {
    let factory = Box::new(TungsteniteFactory {
        ws_url: ws_url_from_api_base(api_base),
        token_provider,
    });
    WsClient::start(factory, config, callbacks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_wss_endpoint_from_https_base() {
        assert_eq!(
            ws_url_from_api_base("https://sandbox.api.knotq.com/"),
            "wss://sandbox.api.knotq.com/v1/sync/ws"
        );
        assert_eq!(
            ws_url_from_api_base("http://localhost:8787"),
            "ws://localhost:8787/v1/sync/ws"
        );
    }

    /// Validate the desktop's OWN tungstenite socket + `connect_workspace_ws`
    /// against a live `wrangler dev` worker (the shared ws_integration test uses a
    /// separate test socket, so this closes the gap on the code that actually
    /// ships). Skips unless KNOTQ_SYNC_BACKEND_URL is set; run with:
    ///   KNOTQ_SYNC_BACKEND_URL=http://127.0.0.1:8788 \
    ///     cargo test -p knotq-app --features ws-sync ws_socket -- --nocapture
    #[test]
    fn connects_and_pulls_against_live_backend() {
        let Ok(base_url) = std::env::var("KNOTQ_SYNC_BACKEND_URL") else {
            println!("[ws_socket] KNOTQ_SYNC_BACKEND_URL not set — skipping live test");
            return;
        };
        let base_url = base_url.trim_end_matches('/').to_string();

        // Bootstrap a sync-entitled test user (KNOTQ_TEST_MODE backend).
        let email = format!("ws-socket-{}@example.com", uuid::Uuid::new_v4());
        let resp = ureq::post(&format!("{base_url}/__test/bootstrap"))
            .send_json(ureq::json!({ "email": email }))
            .expect("bootstrap")
            .into_json::<serde_json::Value>()
            .expect("bootstrap json");
        let token = resp["bearer_token"].as_str().expect("token").to_string();

        let token_provider: TokenProvider = std::sync::Arc::new(move || Some(token.clone()));
        let client = connect_workspace_ws(
            &base_url,
            token_provider,
            knotq_sync::ws::WsConfig::default(),
            knotq_sync::ws::WsCallbacks::noop(),
        );

        // Wait for the socket to come up, then pull over it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline && !client.is_connected() {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(client.is_connected(), "desktop ws socket failed to connect");

        let request = knotq_sync::BatchPullRequest {
            replica_id: knotq_model::ReplicaId::new(),
            cursors: Default::default(),
        };
        let response = client.request_pull(&request);
        assert!(
            response.is_ok(),
            "pull over the desktop ws socket failed: {response:?}"
        );
    }
}
