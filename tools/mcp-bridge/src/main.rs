//! `knotq-mcp` — a stdio front end for the desktop app's loopback MCP server.
//!
//! Most MCP clients today speak stdio and launch the server as a subprocess.
//! KnotQ's server cannot work that way: it has to run *inside* the desktop app,
//! because that is the only place the workspace, the undo stack and the CRDT
//! documents live, and a second process editing the same files would produce
//! exactly the divergence the CRDT design exists to prevent.
//!
//! So this binary is deliberately almost nothing: it reads JSON-RPC frames from
//! stdin, forwards each verbatim to the running app, and writes the reply to
//! stdout. It holds no state, makes no decisions about the protocol, and adds
//! no behaviour a client could come to depend on.
//!
//! Configure a client with:
//!
//! ```json
//! { "command": "knotq-mcp" }
//! ```
//!
//! It finds the app by reading `mcp-endpoint.json` from the KnotQ data
//! directory, which the app rewrites every time it starts and removes when it
//! stops. `KNOTQ_DATA_DIR` overrides the location, exactly as it does for the
//! app itself.

use std::io::{BufRead, Write};

use anyhow::{bail, Context, Result};
use knotq_storage_json::{load_mcp_endpoint, mcp_endpoint_path, McpEndpoint};

/// Generous: a tool call waits on KnotQ's main thread, which may be busy
/// rendering. The app applies its own shorter budget internally.
const REQUEST_TIMEOUT_SECS: u64 = 60;

fn main() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // Resolved per request rather than once at startup: the app may be started,
    // restarted, or have its port change while a long-lived client is attached,
    // and re-reading costs one small file read per call.
    for line in stdin.lock().lines() {
        let line = line.context("read from stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match forward(&line) {
            Ok(Some(response)) => response,
            // A notification: the app answered 202 with no body, and JSON-RPC
            // says to stay silent.
            Ok(None) => continue,
            Err(e) => transport_error(&line, &e.to_string()),
        };
        writeln!(stdout, "{response}").context("write to stdout")?;
        stdout.flush().context("flush stdout")?;
    }
    Ok(())
}

fn forward(request: &str) -> Result<Option<String>> {
    let endpoint = endpoint()?;
    let response = ureq::post(&endpoint.url)
        .set("Authorization", &format!("Bearer {}", endpoint.token))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .send_string(request);

    match response {
        Ok(response) if response.status() == 202 => Ok(None),
        Ok(response) => Ok(Some(response.into_string().context("read the response")?)),
        // A 4xx carries a JSON body explaining itself; pass that on rather than
        // flattening it into "request failed".
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            bail!("KnotQ refused the request ({status}): {body}")
        }
        Err(e) => bail!("could not reach KnotQ: {e}"),
    }
}

fn endpoint() -> Result<McpEndpoint> {
    load_mcp_endpoint().with_context(|| {
        format!(
            "KnotQ does not appear to be running with its MCP server enabled \
             (no readable {}). Open KnotQ and turn the MCP server on in settings.",
            mcp_endpoint_path().display()
        )
    })
}

/// A JSON-RPC error carrying the request's own id, so the client can match it
/// up rather than hanging on a call that will never be answered.
fn transport_error(request: &str, message: &str) -> String {
    let id = serde_json::from_str::<serde_json::Value>(request)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32603, "message": message },
    })
    .to_string()
}
