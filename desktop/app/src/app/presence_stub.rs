//! Presence stubs compiled when the `accounts` feature is off.
//!
//! Multiplayer presence (peer carets) rides on the account sync WebSocket, which
//! is compiled out without `accounts`. The editor render paths still call these
//! two methods, so provide harmless no-op implementations that keep the app
//! rendering with no remote cursors.

use gpui::{App, Entity};
use knotq_editor::{RemoteCursor, SchemeEditor};
use knotq_model::SchemeId;

use super::KnotQApp;

impl KnotQApp {
    /// No peer carets without account sync.
    pub(crate) fn remote_cursors_for_scheme(&self, _scheme: SchemeId) -> Vec<RemoteCursor> {
        Vec::new()
    }

    /// Broadcasting local presence is a no-op without account sync.
    pub(crate) fn send_local_presence(
        &self,
        _scheme: SchemeId,
        _editor: &Entity<SchemeEditor>,
        _cx: &App,
    ) {
    }
}
