//! The loopback listener.
//!
//! Runs on plain OS threads rather than the GPUI executor: `accept` and the
//! per-connection reads block, and blocking a GPUI background task is how you
//! starve every other background task in the app. Connection volume here is a
//! handful at a time, so a thread per connection is the honest, simple choice.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use knotq_mcp::protocol::{self, Routed};

use super::http::{
    admit, json_response, parse_request, rejection_response, ParseError, Rejection, MAX_BODY_BYTES,
};
use super::{McpJob, McpJobSender};

/// A connection that goes quiet mid-request is dropped rather than held. Long
/// enough for any real client, short enough that a stuck peer does not pin a
/// thread forever.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a connection thread waits for the main thread to evaluate a tool
/// call. Generous: the app may legitimately be busy rendering. Bounded so a
/// wedged main thread produces an error the client can see rather than a
/// connection that never answers.
const EVALUATION_TIMEOUT: Duration = Duration::from_secs(20);

pub(super) struct Listening {
    pub listener: TcpListener,
    pub port: u16,
}

/// Bind the configured port, falling back to whatever the OS will give us.
///
/// A fixed port is what lets a client's saved configuration survive a restart,
/// but it is not worth refusing to start over: a stale process or an unrelated
/// service holding the port would otherwise leave the user with a feature that
/// is switched on and silently absent. The endpoint file records where we
/// actually landed.
pub(super) fn bind(preferred: u16) -> std::io::Result<Listening> {
    let loopback = |port: u16| SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpListener::bind(loopback(preferred)) {
        Ok(listener) => Ok(Listening {
            port: listener.local_addr()?.port(),
            listener,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("[mcp] port {preferred} is in use; binding an available port instead");
            let listener = TcpListener::bind(loopback(0))?;
            Ok(Listening {
                port: listener.local_addr()?.port(),
                listener,
            })
        }
        Err(e) => Err(e),
    }
}

/// Serve connections until `shutdown` is set.
pub(super) fn serve(
    listener: TcpListener,
    token: Arc<String>,
    jobs: McpJobSender,
    shutdown: Arc<AtomicBool>,
) {
    for stream in listener.incoming() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        match stream {
            Ok(stream) => {
                let token = Arc::clone(&token);
                let jobs = jobs.clone();
                std::thread::Builder::new()
                    .name("knotq-mcp-conn".into())
                    .spawn(move || handle_connection(stream, &token, &jobs))
                    .ok();
            }
            // One failed accept is not a reason to stop serving; a listener
            // that has actually gone away will keep failing and the shutdown
            // flag will catch it.
            Err(e) => eprintln!("[mcp] accept failed: {e}"),
        }
    }
}

fn handle_connection(mut stream: TcpStream, token: &str, jobs: &McpJobSender) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(rejection) => {
            let _ = stream.write_all(rejection_response(&rejection).as_bytes());
            return;
        }
    };

    if let Err(rejection) = admit(&request, token) {
        let _ = stream.write_all(rejection_response(&rejection).as_bytes());
        return;
    }

    let body = match protocol::parse(&request.body) {
        Ok(parsed) => parsed,
        Err(error_response) => {
            let _ = stream.write_all(
                json_response(200, "OK", &error_response.to_string()).as_bytes(),
            );
            return;
        }
    };

    let response = match protocol::route(&body) {
        // `initialize`, `tools/list` and the rest are answered right here: they
        // are pure protocol and need nothing from the workspace, so they never
        // touch the main thread. Only a tool call does.
        Routed::Respond(response) => Some(response),
        Routed::Silent => None,
        Routed::CallTool {
            id,
            name,
            arguments,
        } => Some(evaluate_on_main_thread(jobs, id, name, arguments)),
    };

    let payload = match response {
        // A JSON-RPC notification gets an HTTP 202 with no body, which is what
        // the Streamable HTTP transport specifies.
        None => "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
        Some(response) => json_response(200, "OK", &response.to_string()),
    };
    let _ = stream.write_all(payload.as_bytes());
    let _ = stream.flush();
}

fn evaluate_on_main_thread(
    jobs: &McpJobSender,
    id: serde_json::Value,
    name: String,
    arguments: Option<serde_json::Value>,
) -> serde_json::Value {
    let (reply, wait) = std::sync::mpsc::sync_channel(1);
    let job = McpJob {
        id: id.clone(),
        name,
        arguments,
        reply,
    };
    if jobs.send_blocking(job).is_err() {
        // The app is shutting down and the receiving task is gone.
        return protocol::internal_error(id, "KnotQ is shutting down");
    }
    match wait.recv_timeout(EVALUATION_TIMEOUT) {
        Ok(response) => response,
        Err(_) => protocol::internal_error(
            id,
            "KnotQ did not answer in time; it may be busy — try again",
        ),
    }
}

/// Read one request: head first, then however much body its `Content-Length`
/// promises.
fn read_request(stream: &mut TcpStream) -> Result<super::http::HttpRequest, Rejection> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let mut request = loop {
        match parse_request(&buffer) {
            Ok(request) => break request,
            Err(ParseError::TooLarge) => return Err(Rejection::TooLarge),
            Err(ParseError::Malformed(_)) => return Err(Rejection::NotFound),
            Err(ParseError::Incomplete) => {}
        }
        match stream.read(&mut chunk) {
            // The peer closed before finishing a request.
            Ok(0) => return Err(Rejection::NotFound),
            Ok(n) => {
                if buffer.len() + n > MAX_BODY_BYTES {
                    return Err(Rejection::TooLarge);
                }
                buffer.extend_from_slice(&chunk[..n]);
            }
            Err(_) => return Err(Rejection::NotFound),
        }
    };

    let Some(expected) = request.content_length() else {
        return Ok(request);
    };
    if expected > MAX_BODY_BYTES {
        return Err(Rejection::TooLarge);
    }
    // The head arrived with only part of the body attached; the rest is still
    // on the wire. A single read is never guaranteed to hold a whole request.
    while request.body.as_bytes().len() < expected {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buffer.len() + n > MAX_BODY_BYTES {
                    return Err(Rejection::TooLarge);
                }
                buffer.extend_from_slice(&chunk[..n]);
                request = parse_request(&buffer).map_err(|_| Rejection::NotFound)?;
            }
            Err(_) => break,
        }
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::*;

    /// A running server plus a stand-in for the GPUI main thread.
    ///
    /// Exercises the real listener, the real connection handling and the real
    /// protocol routing over a real socket — everything except the workspace
    /// itself, which the stub answers for. The parts this covers (framing,
    /// admission, the reply hand-off) are precisely the ones that cannot be
    /// reached from the pure `knotq-mcp` tests.
    struct Harness {
        port: u16,
        token: String,
        shutdown: Arc<AtomicBool>,
    }

    impl Harness {
        /// `answer` plays the main thread: it sees each job and decides the reply.
        fn start(answer: impl Fn(&McpJob) -> serde_json::Value + Send + 'static) -> Self {
            let listening = bind(0).expect("bind an ephemeral port");
            let port = listening.port;
            let token = "test-token-abcdefghijklmnop".to_string();
            let shutdown = Arc::new(AtomicBool::new(false));
            let (tx, rx) = async_channel::bounded::<McpJob>(8);

            let serve_token = Arc::new(token.clone());
            let serve_shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                serve(listening.listener, serve_token, tx, serve_shutdown);
            });
            std::thread::spawn(move || {
                while let Ok(job) = rx.recv_blocking() {
                    let response = answer(&job);
                    let _ = job.reply.send(response);
                }
            });

            Self {
                port,
                token,
                shutdown,
            }
        }

        fn start_echoing_the_tool_name() -> Self {
            Self::start(|job| {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": job.id,
                    "result": { "tool": job.name, "arguments": job.arguments },
                })
            })
        }

        fn request(&self, raw: &str) -> String {
            let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream.write_all(raw.as_bytes()).expect("write");
            stream.flush().unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read");
            response
        }

        fn post(&self, body: &str, extra_headers: &str) -> String {
            self.request(&format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra_headers}Content-Length: {}\r\n\r\n{body}",
                body.as_bytes().len()
            ))
        }

        fn authorized(&self, body: &str) -> String {
            self.post(body, &format!("Authorization: Bearer {}\r\n", self.token))
        }

        fn body_of(response: &str) -> serde_json::Value {
            let body = response
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or("");
            serde_json::from_str(body)
                .unwrap_or_else(|e| panic!("body was not JSON ({e}): {response}"))
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(("127.0.0.1", self.port));
        }
    }

    #[test]
    fn initialize_is_answered_without_ever_reaching_the_main_thread() {
        // The stub panics if called: a handshake must not cost a main-thread hop.
        let harness = Harness::start(|_| panic!("initialize must not reach the workspace"));
        let response = harness.authorized(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        let body = Harness::body_of(&response);
        assert_eq!(body["result"]["serverInfo"]["name"], "knotq");
    }

    #[test]
    fn tools_list_is_also_answered_locally() {
        let harness = Harness::start(|_| panic!("tools/list must not reach the workspace"));
        let body = Harness::body_of(&harness.authorized(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ));
        assert!(!body["result"]["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_tool_call_is_handed_to_the_main_thread_and_its_reply_comes_back() {
        let harness = Harness::start_echoing_the_tool_name();
        let body = Harness::body_of(&harness.authorized(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_schemes","arguments":{"include_archived":true}}}"#,
        ));
        assert_eq!(body["result"]["tool"], "list_schemes");
        assert_eq!(body["result"]["arguments"]["include_archived"], true);
        assert_eq!(body["id"], 3);
    }

    #[test]
    fn a_request_without_a_token_is_refused_at_the_socket() {
        let harness = Harness::start(|_| panic!("an unauthorized request must not be evaluated"));
        let response = harness.post(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#, "");
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    }

    #[test]
    fn a_browser_origin_is_refused_even_with_a_valid_token() {
        let harness = Harness::start(|_| panic!("a rebinding attempt must not be evaluated"));
        let response = harness.post(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &format!(
                "Origin: https://evil.example\r\nAuthorization: Bearer {}\r\n",
                harness.token
            ),
        );
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    }

    #[test]
    fn another_path_is_a_404() {
        let harness = Harness::start_echoing_the_tool_name();
        let response = harness.request(&format!(
            "POST /admin HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\nContent-Length: 2\r\n\r\n{{}}",
            harness.token
        ));
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    }

    /// A single `read` is not guaranteed to hold a whole request, and a body
    /// split across packets is the normal case for anything non-trivial.
    #[test]
    fn a_body_that_arrives_in_pieces_is_reassembled() {
        let harness = Harness::start_echoing_the_tool_name();
        let body = r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"read_scheme","arguments":{"scheme_id":"x"}}}"#;
        let mut stream = TcpStream::connect(("127.0.0.1", harness.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let head = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n",
            harness.token,
            body.len()
        );
        let (first, second) = body.split_at(body.len() / 2);
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(first.as_bytes()).unwrap();
        stream.flush().unwrap();
        // Force a second packet.
        std::thread::yield_now();
        stream.write_all(second.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let parsed = Harness::body_of(&response);
        assert_eq!(parsed["result"]["tool"], "read_scheme");
    }

    #[test]
    fn a_notification_gets_an_empty_202_rather_than_a_response_body() {
        let harness = Harness::start(|_| panic!("a notification must not be evaluated"));
        let response =
            harness.authorized(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(response.starts_with("HTTP/1.1 202"), "{response}");
        assert!(response.ends_with("\r\n\r\n"), "{response:?}");
    }

    #[test]
    fn malformed_json_comes_back_as_a_parse_error_not_a_dropped_connection() {
        let harness = Harness::start_echoing_the_tool_name();
        let body = Harness::body_of(&harness.authorized("{not json"));
        assert_eq!(body["error"]["code"], knotq_mcp::protocol::PARSE_ERROR);
    }

    #[test]
    fn the_server_keeps_serving_after_a_refused_request() {
        let harness = Harness::start_echoing_the_tool_name();
        assert!(harness
            .post(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#, "")
            .starts_with("HTTP/1.1 401"));
        // One bad caller must not take the server down for the good one.
        let body = Harness::body_of(&harness.authorized(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search"}}"#,
        ));
        assert_eq!(body["result"]["tool"], "search");
    }

    #[test]
    fn several_clients_can_be_served_at_once() {
        let harness = Harness::start_echoing_the_tool_name();
        let port = harness.port;
        let token = harness.token.clone();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let token = token.clone();
                std::thread::spawn(move || {
                    let body = format!(
                        r#"{{"jsonrpc":"2.0","id":{i},"method":"tools/call","params":{{"name":"list_schemes"}}}}"#
                    );
                    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    write!(
                        stream,
                        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                    stream.flush().unwrap();
                    let mut response = String::new();
                    stream.read_to_string(&mut response).unwrap();
                    (i, Harness::body_of(&response))
                })
            })
            .collect();
        for handle in handles {
            let (i, body) = handle.join().expect("client thread");
            // Each client gets *its own* answer back, not another's.
            assert_eq!(body["id"], i);
            assert_eq!(body["result"]["tool"], "list_schemes");
        }
    }

    /// The preferred port is what makes a saved client config keep working; the
    /// fallback is what stops a taken port turning the feature into a silent no-op.
    #[test]
    fn binding_falls_back_when_the_preferred_port_is_taken() {
        let squatter = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let taken = squatter.local_addr().unwrap().port();

        let listening = bind(taken).expect("should still bind somewhere");
        assert_ne!(listening.port, taken);
        assert_ne!(listening.port, 0);
    }

    #[test]
    fn binding_returns_the_preferred_port_when_it_is_free() {
        // Take a port, learn its number, then release it.
        let port = {
            let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            probe.local_addr().unwrap().port()
        };
        let listening = bind(port).expect("bind");
        assert_eq!(listening.port, port);
    }
}
