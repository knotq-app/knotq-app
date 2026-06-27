//! WebSocket wire frames for the persistent sync transport.
//!
//! These mirror the backend protocol in `backend/cloudflare/src/workspace_object/
//! socket.ts`. The client multiplexes request/response over one socket (every
//! request carries a monotonic `id` the server echoes) and also receives two
//! server-initiated frames: `changed` (a nudge to pull, replacing online polling)
//! and `presence` (an ephemeral relay, e.g. live cursors — never persisted).
use serde::Deserialize;

use crate::{BatchPullRequest, BatchPushRequest};

/// Literal keepalive text frames. The backend answers `ping` with `pong` via the
/// Durable Object's `setWebSocketAutoResponse`, so a keepalive never wakes the DO
/// from hibernation.
pub const KEEPALIVE_PING: &str = "ping";
pub const KEEPALIVE_PONG: &str = "pong";

/// A frame received from the server. `*_result` carry the response payload as a raw
/// JSON value so the waiting caller deserializes it into the concrete response type
/// (the request id already disambiguates pull vs push).
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerFrame {
    PullResult {
        id: u64,
        res: serde_json::Value,
    },
    PushResult {
        id: u64,
        res: serde_json::Value,
    },
    /// A request-scoped error (carries the originating `id`) or a connection-level
    /// error (no `id`). `error` holds a backend error code (e.g.
    /// `crdt_schema_invalid`); `code` holds protocol errors (`bad_frame`, …).
    Error {
        #[serde(default)]
        id: Option<u64>,
        #[serde(default)]
        status: Option<u16>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        code: Option<String>,
    },
    /// Server nudge after some other device pushed: pull to converge.
    Changed {
        #[serde(default)]
        documents: u64,
        #[serde(default)]
        notification_schedule_revision: u64,
    },
    /// Ephemeral presence relayed from another device (e.g. a live cursor).
    Presence {
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        data: Option<serde_json::Value>,
    },
}

impl ServerFrame {
    /// Parse a text frame. Returns `None` for the literal `pong` keepalive ack and
    /// for anything that doesn't deserialize (caller ignores those).
    pub fn parse(text: &str) -> Option<ServerFrame> {
        if text == KEEPALIVE_PONG {
            return None;
        }
        serde_json::from_str(text).ok()
    }
}

/// Build the JSON text for a pull request frame.
pub fn build_pull_frame(id: u64, req: &BatchPullRequest) -> String {
    serde_json::json!({ "t": "pull", "id": id, "req": req }).to_string()
}

/// Build the JSON text for a push request frame.
pub fn build_push_frame(id: u64, req: &BatchPushRequest) -> String {
    serde_json::json!({ "t": "push", "id": id, "req": req }).to_string()
}

/// Build the JSON text for an ephemeral presence frame (fire-and-forget).
pub fn build_presence_frame(data: Option<serde_json::Value>) -> String {
    serde_json::json!({ "t": "presence", "data": data }).to_string()
}
