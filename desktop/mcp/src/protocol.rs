//! JSON-RPC 2.0 framing and the handful of MCP methods this server answers.
//!
//! Kept separate from transport so the whole protocol surface — including the
//! error paths a client is most likely to hit and least likely to be tested
//! against — can be exercised without opening a socket.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Outcome, ToolError};

/// The MCP revision this server implements.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "knotq";

// JSON-RPC reserved codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub jsonrpc: String,
    /// Absent on a notification, which by JSON-RPC rules gets no response.
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl Request {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// What the transport has to do with a parsed request.
#[derive(Debug)]
pub enum Routed {
    /// Answer with this, as-is.
    Respond(Value),
    /// Send nothing at all (a JSON-RPC notification).
    Silent,
    /// Run this tool against the live workspace, then wrap the outcome with
    /// [`tool_result`] and answer with [`success`].
    CallTool {
        id: Value,
        name: String,
        arguments: Option<Value>,
    },
}

/// Decide what a request means, without touching any workspace state.
pub fn route(request: &Request) -> Routed {
    // JSON-RPC requires the version tag, but rejecting a client that omits it
    // buys nothing and breaks hand-written scripts; only a *wrong* version is a
    // real signal that something else is on the wire.
    if !request.jsonrpc.is_empty() && request.jsonrpc != "2.0" {
        return match &request.id {
            Some(id) => Routed::Respond(error(
                id.clone(),
                INVALID_REQUEST,
                "only JSON-RPC 2.0 is supported",
            )),
            None => Routed::Silent,
        };
    }

    // Notifications (`notifications/initialized`, `notifications/cancelled`)
    // are acknowledged by silence. Answering one is a protocol violation that
    // some clients treat as fatal.
    let Some(id) = request.id.clone() else {
        return Routed::Silent;
    };

    match request.method.as_str() {
        "initialize" => Routed::Respond(success(id, initialize_result())),
        "ping" => Routed::Respond(success(id, json!({}))),
        "tools/list" => Routed::Respond(success(id, tools_list_result())),
        "tools/call" => {
            let params = request.params.as_ref();
            let Some(name) = params
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
            else {
                return Routed::Respond(error(
                    id,
                    INVALID_REQUEST,
                    "tools/call requires a `name`",
                ));
            };
            Routed::CallTool {
                id,
                name: name.to_string(),
                arguments: params.and_then(|p| p.get("arguments")).cloned(),
            }
        }
        // Advertising no prompts/resources capability should stop a client from
        // asking, but several ask anyway; an empty list is friendlier than an
        // error and costs nothing.
        "prompts/list" => Routed::Respond(success(id, json!({ "prompts": [] }))),
        "resources/list" => Routed::Respond(success(id, json!({ "resources": [] }))),
        "resources/templates/list" => {
            Routed::Respond(success(id, json!({ "resourceTemplates": [] })))
        }
        other => Routed::Respond(error(
            id,
            METHOD_NOT_FOUND,
            format!("unsupported method `{other}`"),
        )),
    }
}

pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "When the user mentions KnotQ or their plans, use these tools first—call list_schemes before answering. Do not launch, inspect, or modify the KnotQ desktop app directly. KnotQ is the user's planning workspace: hierarchical documents \
                         (schemes) of lines (items), where a line can carry dates, a repeat \
                         rule, a priority and a completion state. Call list_schemes first to \
                         learn ids; ids are opaque, so pass them back exactly as given. Use \
                         list_upcoming for \"what's next\" and list_calendar for a date range. \
                         Writes go through the same path as the user's own edits, so they \
                         undo and sync normally.",
    })
}

pub fn tools_list_result() -> Value {
    json!({ "tools": crate::tool_definitions() })
}

/// Wrap a tool outcome in the MCP `tools/call` result shape.
///
/// The payload is sent twice on purpose: `structuredContent` for clients that
/// read typed results, and the same JSON as text for the many that only read
/// `content`. Dropping either one makes the tool invisible to half the clients
/// in the wild.
pub fn tool_result(payload: &Value, changed: bool) -> Value {
    let mut payload = payload.clone();
    if let Value::Object(map) = &mut payload {
        map.insert("changed".to_string(), Value::Bool(changed));
    }
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()),
        }],
        "structuredContent": payload,
        "isError": false,
    })
}

/// Wrap a refusal as a *successful* call carrying `isError: true`.
///
/// This is the MCP convention, and the distinction matters: a read-only scheme
/// or a stale id is an answer the model should read and adapt to, whereas a
/// JSON-RPC error is a transport-level failure most clients surface to the user
/// and do not let the model see.
pub fn tool_error_result(err: &ToolError) -> Value {
    json!({
        "content": [{ "type": "text", "text": err.to_string() }],
        "structuredContent": { "error": err.kind(), "message": err.to_string() },
        "isError": true,
    })
}

/// The response for a completed tool call, whichever way it went.
pub fn tool_call_response(id: Value, result: Result<Outcome, ToolError>) -> Value {
    match result {
        Ok(Outcome::Read(value)) => success(id, tool_result(&value, false)),
        Ok(Outcome::Unchanged(value)) => success(id, tool_result(&value, false)),
        // A `Write` reaching here un-applied is a bug in the transport, not
        // something a client can cause.
        Ok(Outcome::Write { response, .. }) => success(id, tool_result(&response, true)),
        Err(err) if err.is_protocol_error() => error(id, err.rpc_code(), err.to_string()),
        Err(err) => success(id, tool_error_result(&err)),
    }
}

/// The response for a write the caller has already applied.
///
/// Separate from [`tool_call_response`] because only the caller knows whether
/// the apply actually changed anything: a command can be filtered out on its
/// way through (a recurrence toggle that resolves to a no-op), and reporting
/// `changed: true` for it would tell the agent it had done something it had not.
pub fn applied_write_response(id: Value, response: &Value, changed: bool) -> Value {
    success(id, tool_result(response, changed))
}

pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

/// A parse failure has no id to answer against, so JSON-RPC says to use null.
pub fn parse_error(message: impl Into<String>) -> Value {
    error(Value::Null, PARSE_ERROR, message)
}

pub fn internal_error(id: Value, message: impl Into<String>) -> Value {
    error(id, INTERNAL_ERROR, message)
}

/// Parse one request frame.
pub fn parse(body: &str) -> Result<Request, Value> {
    serde_json::from_str::<Request>(body)
        .map_err(|e| parse_error(format!("could not parse JSON-RPC request: {e}")))
}

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: &'static str,
}
