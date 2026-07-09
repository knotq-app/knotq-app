//! Platform-agnostic WebSocket sync client.
//!
//! This is the *core* of the WebSocket transport: connection supervision
//! (reconnect with exponential backoff), request/response multiplexing over one
//! socket (callers block for the reply matching their request id), a hibernation
//! keepalive, and delivery of server-initiated `changed`/`presence` frames to
//! callbacks. It is generic over a [`RawSocket`] the platform provides (desktop:
//! tungstenite; mobile: the same), so the tricky concurrency lives here once and is
//! unit-tested with an in-memory fake — no live server, no GPUI, no tokio.
//!
//! The actual `SyncTransport` adapter (mapping [`WsRequestError`] to each
//! platform's error types — e.g. desktop's `SyncNetworkUnreachable`, and the
//! engine's `SyncPushRejected` for push) lives in the platform crate, because those
//! error types are platform-local.
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{BatchPullResponse, BatchPushResponse};

use super::frames::{self, ServerFrame, KEEPALIVE_PING};

/// A minimal duplex text socket the platform implements. All KnotQ clients are
/// native, so the concrete impl is a blocking WebSocket (tungstenite) with a read
/// timeout used to interleave reads with outbound sends on one thread.
pub trait RawSocket: Send {
    /// Block up to `timeout` for one inbound text message. `Ok(None)` on timeout
    /// (nothing arrived); `Err` means the connection is broken (triggers reconnect).
    fn poll(&mut self, timeout: Duration) -> io::Result<Option<String>>;
    /// Send one text message.
    fn send(&mut self, text: &str) -> io::Result<()>;
    /// Close the connection (best effort).
    fn close(&mut self);
}

/// Establishes a connection. Called by the supervisor on first connect and on every
/// reconnect, so it must re-resolve auth/URL each time (a desktop impl captures a
/// token provider, not a fixed token).
pub trait RawSocketFactory: Send + 'static {
    fn connect(&self) -> io::Result<Box<dyn RawSocket>>;
}

/// Why a request could not be completed over the socket. The platform adapter maps
/// these onto its `SyncTransport` error contract.
#[derive(Debug, Clone)]
pub enum WsRequestError {
    /// No live socket right now — the caller should fall back to HTTP.
    NotConnected,
    /// The socket dropped while the request was in flight.
    Disconnected,
    /// No reply within the request timeout.
    Timeout,
    /// The server returned an error frame for this request. `code` is the backend
    /// error code (e.g. `crdt_schema_invalid`); on push the adapter turns this into
    /// the engine's `SyncPushRejected` so its self-heal fires.
    Server { status: Option<u16>, code: String },
    /// The reply payload didn't deserialize into the expected response type.
    Decode(String),
}

impl std::fmt::Display for WsRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WsRequestError::NotConnected => write!(f, "websocket sync not connected"),
            WsRequestError::Disconnected => write!(f, "websocket sync connection dropped"),
            WsRequestError::Timeout => write!(f, "websocket sync request timed out"),
            WsRequestError::Server { status, code } => {
                write!(f, "websocket sync server error {status:?}: {code}")
            }
            WsRequestError::Decode(msg) => write!(f, "websocket sync decode error: {msg}"),
        }
    }
}

impl std::error::Error for WsRequestError {}

/// An ephemeral presence frame relayed from another device.
#[derive(Debug, Clone)]
pub struct PresenceEvent {
    pub from: Option<String>,
    pub data: Option<serde_json::Value>,
}

/// Tunables. Defaults suit a desktop foreground connection.
#[derive(Debug, Clone)]
pub struct WsConfig {
    /// How long the session thread blocks for an inbound message before looping to
    /// drain outbound sends. Bounds the added send latency. Keep small.
    pub poll_interval: Duration,
    /// How long `request_*` blocks for a reply before returning `Timeout`.
    pub request_timeout: Duration,
    /// Send a keepalive ping this often (answered by the runtime's auto-response
    /// without waking the DO, so it's nearly free). MUST be shorter than the
    /// shortest idle timeout anywhere on the connection path — empirically a
    /// sandbox socket is dropped (1006) ~13-15 s after the last frame, so a 30 s
    /// keepalive let an idle socket die between pings. This matters most now that
    /// the foreground does no polling: the keepalive is the *only* idle traffic.
    pub keepalive_interval: Duration,
    /// First reconnect backoff after an unstable/failed session.
    pub initial_backoff: Duration,
    /// Cap on reconnect backoff.
    pub max_backoff: Duration,
    /// A session that stayed up at least this long is "stable": the next reconnect
    /// uses no backoff. Shorter-lived sessions are treated as flapping and backed off.
    pub stable_after: Duration,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(50),
            request_timeout: Duration::from_secs(30),
            // Well under the observed ~13-15 s idle drop, with margin for jitter.
            keepalive_interval: Duration::from_secs(8),
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            stable_after: Duration::from_secs(10),
        }
    }
}

/// Callbacks for server-initiated frames. Both run on the session thread, so keep
/// them quick (e.g. signal a channel / set a flag) and non-blocking.
pub struct WsCallbacks {
    /// A peer pushed: run a sync to converge. Replaces the online poll.
    pub on_changed: Box<dyn Fn() + Send + Sync>,
    /// A peer's presence update (e.g. a live cursor) arrived.
    pub on_presence: Box<dyn Fn(PresenceEvent) + Send + Sync>,
    /// A session just became connected (first connect *or* a reconnect). The
    /// platform should run a catch-up sync: any `changed` nudge broadcast while the
    /// socket was down was missed, so without polling this is what re-converges a
    /// device after a transient drop. Fires on every (re)connect; a redundant
    /// startup sync is harmless (idempotent).
    pub on_connect: Box<dyn Fn() + Send + Sync>,
}

impl WsCallbacks {
    /// Callbacks that do nothing — handy for tests or a presence-less client.
    pub fn noop() -> Self {
        Self {
            on_changed: Box::new(|| {}),
            on_presence: Box::new(|_| {}),
            on_connect: Box::new(|| {}),
        }
    }
}

type ReplyResult = Result<serde_json::Value, WsRequestError>;

struct Shared {
    config: WsConfig,
    pending: Mutex<HashMap<u64, SyncSender<ReplyResult>>>,
    /// `Some` while a session is live; the session thread owns the receiver.
    outgoing: Mutex<Option<Sender<String>>>,
    connected: AtomicBool,
    next_id: AtomicU64,
    stop: AtomicBool,
    callbacks: WsCallbacks,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A persistent WebSocket sync connection. Spawns a background supervisor thread
/// that keeps a socket connected; `request_pull`/`request_push` block for the reply.
pub struct WsClient {
    shared: Arc<Shared>,
    _supervisor: Option<JoinHandle<()>>,
}

impl WsClient {
    /// Start the client: spawns the supervisor thread, which connects via `factory`
    /// and reconnects with backoff for the client's lifetime (until dropped).
    pub fn start(
        factory: Box<dyn RawSocketFactory>,
        config: WsConfig,
        callbacks: WsCallbacks,
    ) -> WsClient {
        let shared = Arc::new(Shared {
            config,
            pending: Mutex::new(HashMap::new()),
            outgoing: Mutex::new(None),
            connected: AtomicBool::new(false),
            next_id: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            callbacks,
        });
        let supervisor = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("knotq-ws-sync".into())
                .spawn(move || run_supervisor(shared, factory))
                .expect("spawn ws supervisor thread")
        };
        WsClient {
            shared,
            _supervisor: Some(supervisor),
        }
    }

    /// True while a socket is live. The platform uses this to choose WS vs HTTP.
    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::SeqCst)
    }

    /// Send a pull over the socket and block for the typed response.
    pub fn request_pull(
        &self,
        req: &crate::BatchPullRequest,
    ) -> Result<BatchPullResponse, WsRequestError> {
        let value = self.request(|id| frames::build_pull_frame(id, req))?;
        serde_json::from_value(value).map_err(|e| WsRequestError::Decode(e.to_string()))
    }

    /// Send a push over the socket and block for the typed response.
    pub fn request_push(
        &self,
        req: &crate::BatchPushRequest,
    ) -> Result<BatchPushResponse, WsRequestError> {
        let value = self.request(|id| frames::build_push_frame(id, req))?;
        serde_json::from_value(value).map_err(|e| WsRequestError::Decode(e.to_string()))
    }

    /// Fire-and-forget an ephemeral presence update (e.g. a live cursor). Dropped
    /// silently when not connected — presence is best-effort.
    pub fn send_presence(&self, data: serde_json::Value) -> Result<(), WsRequestError> {
        if !self.is_connected() {
            return Err(WsRequestError::NotConnected);
        }
        let frame = frames::build_presence_frame(Some(data));
        match lock(&self.shared.outgoing).as_ref() {
            Some(tx) => tx.send(frame).map_err(|_| WsRequestError::NotConnected),
            None => Err(WsRequestError::NotConnected),
        }
    }

    /// Register a request id, send its frame, and block for the matching reply.
    fn request(&self, build: impl FnOnce(u64) -> String) -> ReplyResult {
        if !self.is_connected() {
            return Err(WsRequestError::NotConnected);
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (reply_tx, reply_rx) = mpsc::sync_channel::<ReplyResult>(1);
        lock(&self.shared.pending).insert(id, reply_tx);

        let frame = build(id);
        let sent = match lock(&self.shared.outgoing).as_ref() {
            Some(tx) => tx.send(frame).is_ok(),
            None => false,
        };
        if !sent {
            lock(&self.shared.pending).remove(&id);
            return Err(WsRequestError::NotConnected);
        }

        match reply_rx.recv_timeout(self.shared.config.request_timeout) {
            Ok(result) => result,
            Err(_) => {
                lock(&self.shared.pending).remove(&id);
                Err(WsRequestError::Timeout)
            }
        }
    }

    /// Stop the supervisor and tear down the socket. Idempotent.
    pub fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for WsClient {
    fn drop(&mut self) {
        self.shutdown();
        // Detach the supervisor: it observes `stop` within one poll interval and
        // exits on its own, so we don't block the dropping thread on a join.
        self._supervisor.take();
    }
}

fn run_supervisor(shared: Arc<Shared>, factory: Box<dyn RawSocketFactory>) {
    // Run the socket I/O + reply delivery at user-initiated QoS so an Apple caller
    // (e.g. the iOS bridge's user-initiated queue, or GPUI's foreground work)
    // blocking on this thread's reply isn't a priority inversion — the OS would
    // otherwise leave this thread at a lower QoS and starve it, stalling every WS
    // request. No-op off Apple platforms.
    set_user_initiated_qos();
    let mut backoff = shared.config.initial_backoff;
    while !shared.stop.load(Ordering::SeqCst) {
        let stable = match factory.connect() {
            Ok(socket) => {
                let started = Instant::now();
                run_session(&shared, socket);
                started.elapsed() >= shared.config.stable_after
            }
            Err(_) => false,
        };
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        if stable {
            // The session stayed up; reconnect promptly.
            backoff = shared.config.initial_backoff;
        } else {
            sleep_interruptible(&shared, backoff);
            backoff = (backoff * 2).min(shared.config.max_backoff);
        }
    }
}

/// Raise the current thread to user-initiated QoS on Apple platforms so callers
/// blocking on its replies don't hit a priority inversion. Elsewhere: a no-op.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn set_user_initiated_qos() {
    // `QOS_CLASS_USER_INITIATED` from <sys/qos.h>; `pthread_set_qos_class_self_np`
    // lives in libSystem (always linked on Apple), so we declare it directly rather
    // than depend on a libc that may not export it.
    const QOS_CLASS_USER_INITIATED: std::os::raw::c_uint = 0x19;
    extern "C" {
        fn pthread_set_qos_class_self_np(
            qos_class: std::os::raw::c_uint,
            relative_priority: std::os::raw::c_int,
        ) -> std::os::raw::c_int;
    }
    // SAFETY: documented libSystem call with no preconditions; result ignored.
    unsafe {
        let _ = pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn set_user_initiated_qos() {}

fn run_session(shared: &Arc<Shared>, mut socket: Box<dyn RawSocket>) {
    let (out_tx, out_rx) = mpsc::channel::<String>();
    *lock(&shared.outgoing) = Some(out_tx);
    shared.connected.store(true, Ordering::SeqCst);
    // Catch-up hook: the platform runs a sync now so a `changed` missed while the
    // socket was down (or before this first connect) is reconciled without polling.
    (shared.callbacks.on_connect)();
    let mut last_keepalive = Instant::now();

    let result = (|| -> io::Result<()> {
        loop {
            if shared.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            // Drain any queued outbound frames.
            while let Ok(text) = out_rx.try_recv() {
                socket.send(&text)?;
            }
            // Hibernation-friendly keepalive.
            if last_keepalive.elapsed() >= shared.config.keepalive_interval {
                socket.send(KEEPALIVE_PING)?;
                last_keepalive = Instant::now();
            }
            // Wait briefly for an inbound frame.
            match socket.poll(shared.config.poll_interval)? {
                Some(text) => handle_incoming(shared, &text),
                None => {}
            }
        }
    })();
    let _ = result;

    // Session over: stop accepting sends, fail in-flight requests, close.
    shared.connected.store(false, Ordering::SeqCst);
    *lock(&shared.outgoing) = None;
    socket.close();
    fail_all_pending(shared, WsRequestError::Disconnected);
}

fn handle_incoming(shared: &Arc<Shared>, text: &str) {
    let Some(frame) = ServerFrame::parse(text) else {
        return; // keepalive pong or unparseable — ignore
    };
    match frame {
        ServerFrame::PullResult { id, res } | ServerFrame::PushResult { id, res } => {
            complete(shared, id, Ok(res));
        }
        ServerFrame::Error {
            id: Some(id),
            status,
            error,
            code,
        } => {
            let code = error
                .or(code)
                .unwrap_or_else(|| "unknown_error".to_string());
            complete(shared, id, Err(WsRequestError::Server { status, code }));
        }
        ServerFrame::Error { id: None, .. } => {
            // Connection-level error with no request id; nothing to complete.
        }
        ServerFrame::Changed { .. } => {
            (shared.callbacks.on_changed)();
        }
        ServerFrame::Presence { from, data } => {
            (shared.callbacks.on_presence)(PresenceEvent { from, data });
        }
    }
}

fn complete(shared: &Arc<Shared>, id: u64, result: ReplyResult) {
    if let Some(reply_tx) = lock(&shared.pending).remove(&id) {
        // The receiver may have already given up (timeout); ignore a send failure.
        let _ = reply_tx.send(result);
    }
}

fn fail_all_pending(shared: &Arc<Shared>, error: WsRequestError) {
    let pending: Vec<_> = lock(&shared.pending).drain().collect();
    for (_id, reply_tx) in pending {
        let _ = reply_tx.send(Err(error.clone()));
    }
}

fn sleep_interruptible(shared: &Arc<Shared>, total: Duration) {
    // Sleep in small chunks so `stop`/shutdown is observed promptly.
    let chunk = Duration::from_millis(50);
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let step = remaining.min(chunk);
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::Receiver as StdReceiver;
    use std::sync::mpsc::{Receiver, RecvTimeoutError};

    /// One end the test acts as the server on, paired with a `FakeSocket`.
    struct ServerHandle {
        to_client: Sender<String>,
        from_client: StdReceiver<String>,
    }

    struct FakeSocket {
        inbound: Receiver<String>,
        outbound: Sender<String>,
    }

    impl RawSocket for FakeSocket {
        fn poll(&mut self, timeout: Duration) -> io::Result<Option<String>> {
            match self.inbound.recv_timeout(timeout) {
                Ok(text) => Ok(Some(text)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
                }
            }
        }
        fn send(&mut self, text: &str) -> io::Result<()> {
            self.outbound
                .send(text.to_string())
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }
        fn close(&mut self) {}
    }

    struct FakeFactory {
        servers: Arc<Mutex<Vec<ServerHandle>>>,
        fail_first: Arc<AtomicUsize>,
    }

    impl RawSocketFactory for FakeFactory {
        fn connect(&self) -> io::Result<Box<dyn RawSocket>> {
            if self.fail_first.load(Ordering::SeqCst) > 0 {
                self.fail_first.fetch_sub(1, Ordering::SeqCst);
                return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "refused"));
            }
            let (to_client_tx, to_client_rx) = mpsc::channel::<String>();
            let (from_client_tx, from_client_rx) = mpsc::channel::<String>();
            lock(&self.servers).push(ServerHandle {
                to_client: to_client_tx,
                from_client: from_client_rx,
            });
            Ok(Box::new(FakeSocket {
                inbound: to_client_rx,
                outbound: from_client_tx,
            }))
        }
    }

    fn fast_config() -> WsConfig {
        WsConfig {
            poll_interval: Duration::from_millis(5),
            request_timeout: Duration::from_secs(2),
            keepalive_interval: Duration::from_secs(3600),
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(50),
            stable_after: Duration::from_millis(10_000),
        }
    }

    fn start_with(
        callbacks: WsCallbacks,
    ) -> (WsClient, Arc<Mutex<Vec<ServerHandle>>>, Arc<AtomicUsize>) {
        let servers = Arc::new(Mutex::new(Vec::new()));
        let fail_first = Arc::new(AtomicUsize::new(0));
        let factory = Box::new(FakeFactory {
            servers: Arc::clone(&servers),
            fail_first: Arc::clone(&fail_first),
        });
        let client = WsClient::start(factory, fast_config(), callbacks);
        (client, servers, fail_first)
    }

    /// Wait until at least `n` server handles exist, returning the last one.
    fn wait_for_server(servers: &Arc<Mutex<Vec<ServerHandle>>>, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if lock(servers).len() >= n {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("server handle {n} never appeared");
    }

    fn wait_connected(client: &WsClient) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if client.is_connected() {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("client never connected");
    }

    fn frame_id(text: &str) -> u64 {
        serde_json::from_str::<serde_json::Value>(text).unwrap()["id"]
            .as_u64()
            .unwrap()
    }

    #[test]
    fn round_trips_a_pull_request() {
        let (client, servers, _) = start_with(WsCallbacks::noop());
        wait_connected(&client);

        let handle = thread::spawn(move || {
            let req = crate::BatchPullRequest {
                replica_id: crate::ReplicaId::new(),
                cursors: Default::default(),
            };
            client.request_pull(&req)
        });

        wait_for_server(&servers, 1);
        // Read the client's pull frame, reply with a matching pull_result.
        let req_text = {
            let guard = lock(&servers);
            guard[0]
                .from_client
                .recv_timeout(Duration::from_secs(2))
                .expect("pull frame")
        };
        let id = frame_id(&req_text);
        let reply = serde_json::json!({
            "t": "pull_result",
            "id": id,
            "res": { "documents": [], "notification_schedule_revision": 0, "has_more": false }
        })
        .to_string();
        lock(&servers)[0].to_client.send(reply).unwrap();

        let response = handle.join().unwrap().expect("pull succeeds");
        assert!(response.documents.is_empty());
        assert!(!response.has_more);
    }

    #[test]
    fn delivers_changed_and_presence_to_callbacks() {
        let changed = Arc::new(AtomicUsize::new(0));
        let presence = Arc::new(Mutex::new(Vec::<PresenceEvent>::new()));
        let callbacks = WsCallbacks {
            on_changed: {
                let changed = Arc::clone(&changed);
                Box::new(move || {
                    changed.fetch_add(1, Ordering::SeqCst);
                })
            },
            on_presence: {
                let presence = Arc::clone(&presence);
                Box::new(move |event| lock(&presence).push(event))
            },
            on_connect: Box::new(|| {}),
        };
        let (client, servers, _) = start_with(callbacks);
        wait_connected(&client);
        wait_for_server(&servers, 1);

        lock(&servers)[0]
            .to_client
            .send(serde_json::json!({ "t": "changed", "documents": 2 }).to_string())
            .unwrap();
        lock(&servers)[0]
            .to_client
            .send(
                serde_json::json!({ "t": "presence", "from": "r1", "data": { "caret": 7 } })
                    .to_string(),
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if changed.load(Ordering::SeqCst) >= 1 && !lock(&presence).is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(changed.load(Ordering::SeqCst), 1);
        let events = lock(&presence);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].from.as_deref(), Some("r1"));
        assert_eq!(events[0].data.as_ref().unwrap()["caret"], 7);
    }

    #[test]
    fn maps_a_server_error_frame_to_a_server_error() {
        let (client, servers, _) = start_with(WsCallbacks::noop());
        wait_connected(&client);

        let handle = thread::spawn(move || {
            let req = crate::BatchPushRequest {
                replica_id: crate::ReplicaId::new(),
                documents: Vec::new(),
                notification_schedule_changed: false,
                notification_schedule: None,
            };
            client.request_push(&req)
        });

        wait_for_server(&servers, 1);
        let req_text = {
            let guard = lock(&servers);
            guard[0]
                .from_client
                .recv_timeout(Duration::from_secs(2))
                .expect("push frame")
        };
        let id = frame_id(&req_text);
        let reply = serde_json::json!({
            "t": "error", "id": id, "status": 400, "error": "crdt_schema_invalid"
        })
        .to_string();
        lock(&servers)[0].to_client.send(reply).unwrap();

        let err = handle.join().unwrap().expect_err("push should error");
        match err {
            WsRequestError::Server { status, code } => {
                assert_eq!(status, Some(400));
                assert_eq!(code, "crdt_schema_invalid");
            }
            other => panic!("expected Server error, got {other:?}"),
        }
    }

    #[test]
    fn reconnects_after_a_disconnect_and_serves_again() {
        let (client, servers, _) = start_with(WsCallbacks::noop());
        wait_connected(&client);
        wait_for_server(&servers, 1);

        // Drop the first server's send half so the client's poll sees a broken pipe.
        {
            let mut guard = lock(&servers);
            let first = guard.remove(0);
            drop(first); // closes both channel ends -> client session ends
        }

        // The supervisor reconnects: a second server handle appears.
        wait_for_server(&servers, 1);
        wait_connected(&client);

        // A request now succeeds over the new session.
        let client = Arc::new(client);
        let req_client = Arc::clone(&client);
        let handle = thread::spawn(move || {
            let req = crate::BatchPullRequest {
                replica_id: crate::ReplicaId::new(),
                cursors: Default::default(),
            };
            req_client.request_pull(&req)
        });

        // Serve the reply on whichever (the current) server handle is live.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut served = false;
        while Instant::now() < deadline && !served {
            let req_text = {
                let guard = lock(&servers);
                guard
                    .last()
                    .and_then(|h| h.from_client.recv_timeout(Duration::from_millis(20)).ok())
            };
            if let Some(req_text) = req_text {
                let id = frame_id(&req_text);
                let reply = serde_json::json!({
                    "t": "pull_result", "id": id,
                    "res": { "documents": [], "notification_schedule_revision": 0, "has_more": false }
                })
                .to_string();
                lock(&servers)
                    .last()
                    .unwrap()
                    .to_client
                    .send(reply)
                    .unwrap();
                served = true;
            }
        }
        assert!(served, "second session never received the request");
        assert!(handle.join().unwrap().is_ok());
    }
}
