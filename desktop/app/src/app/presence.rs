//! Live multiplayer presence (peer carets) over the WebSocket sync channel.
//!
//! Sending: on a local cursor move we broadcast `{scheme, item, col, replica}` as
//! an ephemeral presence frame (never persisted). Receiving: peers' frames arrive
//! on the ws `on_presence` callback, are funnelled through an async channel to the
//! GPUI thread, and stored per-scheme keyed by replica. The daily-queue / scheme
//! render passes them into the editor as `RemoteCursor`s. All best-effort and a
//! no-op when the socket is down (so the default, non-`ws-sync` build does nothing).
use std::time::{Duration, Instant};

use gpui::{App, Entity};
use knotq_editor::{RemoteCursor, SchemeEditor};
use knotq_model::{ItemId, SchemeId};
use knotq_sync::ws::PresenceEvent;

use super::KnotQApp;

/// Distinct, theme-agnostic peer colors (RGBA tokens). Assigned by hashing the
/// peer's replica id so a given device keeps a stable color across the session.
const PEER_PALETTE: [u32; 6] = [
    0xE5484DFF, // red
    0x0091FFFF, // blue
    0x30A46CFF, // green
    0xF76B15FF, // orange
    0x8E4EC6FF, // purple
    0xE2A610FF, // amber
];

/// A peer caret stops being drawn this long after its last update (the peer went
/// idle or disconnected without a clean signal).
const PRESENCE_TTL: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct PeerCursor {
    pub replica: String,
    pub item_id: ItemId,
    pub col: usize,
    pub color: u32,
    pub at: Instant,
}

fn color_for_replica(replica: &str) -> u32 {
    // FNV-1a → palette index (stable per replica).
    let mut hash: u32 = 0x811c_9dc5;
    for byte in replica.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    PEER_PALETTE[(hash as usize) % PEER_PALETTE.len()]
}

impl KnotQApp {
    /// Apply one received presence frame: record the peer's caret on its scheme and
    /// remove its caret from every other scheme (a peer has exactly one caret).
    pub(crate) fn apply_presence_event(&mut self, event: PresenceEvent) {
        let Some(data) = event.data else {
            return;
        };
        let scheme = data
            .get("scheme")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<SchemeId>().ok());
        let item = data
            .get("item")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<ItemId>().ok());
        let col = data.get("col").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        // Prefer the self-reported replica (always present); fall back to the
        // server-stamped `from`.
        let replica = data
            .get("replica")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or(event.from)
            .unwrap_or_default();
        let (Some(scheme), Some(item)) = (scheme, item) else {
            return;
        };
        if replica.is_empty() {
            return;
        }
        let color = color_for_replica(&replica);
        // Drop this peer's caret from any scheme it was previously on.
        for cursors in self.presence_cursors.values_mut() {
            cursors.retain(|cursor| cursor.replica != replica);
        }
        self.presence_cursors
            .entry(scheme)
            .or_default()
            .push(PeerCursor {
                replica,
                item_id: item,
                col,
                color,
                at: Instant::now(),
            });
    }

    /// The non-expired peer carets for a scheme, as editor `RemoteCursor`s.
    pub(crate) fn remote_cursors_for_scheme(&self, scheme: SchemeId) -> Vec<RemoteCursor> {
        let now = Instant::now();
        self.presence_cursors
            .get(&scheme)
            .map(|cursors| {
                cursors
                    .iter()
                    .filter(|cursor| now.duration_since(cursor.at) < PRESENCE_TTL)
                    .map(|cursor| RemoteCursor {
                        item_id: cursor.item_id,
                        col: cursor.col,
                        color: cursor.color,
                        label: cursor.replica.chars().take(4).collect(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Broadcast the local caret in `scheme` over the socket (best-effort; no-op
    /// when the socket is down or the caret isn't on a known item).
    pub(crate) fn send_local_presence(
        &self,
        scheme: SchemeId,
        editor: &Entity<SchemeEditor>,
        cx: &App,
    ) {
        let Some(ws) = self.ws_sync.as_ref() else {
            return;
        };
        if !ws.is_connected() {
            return;
        }
        let Some((item_id, col)) = editor.read(cx).caret_presence() else {
            return;
        };
        let data = serde_json::json!({
            "scheme": scheme.to_string(),
            "item": item_id.to_string(),
            "col": col,
            "replica": self.settings.replica_id.to_string(),
        });
        let _ = ws.send_presence(data);
    }
}
