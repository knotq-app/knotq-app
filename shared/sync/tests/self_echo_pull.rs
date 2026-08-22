//! Pins the "self-echo" shape of the sync protocol.
//!
//! A push advances the server's document `seq` but never the pusher's own pull
//! cursor, so the very next pull hands the pushing device the document it just
//! sent. That round trip carries no new information, yet on the desktop it used
//! to land as a full workspace replace, a scroll restore and a whole-window
//! repaint — continuously, while typing over a live socket.
//!
//! The desktop fix is to notice that such a run moved nothing and skip the UI
//! work (see `replace_workspace_from_sync`, which reports whether anything
//! visible changed). These tests exist so the *shape* the fix relies on stays
//! true: if a future change makes a push advance the pull cursor, the echo
//! disappears and these tests say so loudly rather than silently.

mod common;

use common::{Harness, D0};

/// After a device pushes, its pull cursor is behind the server head — so the
/// next pull re-downloads the device's own document.
#[test]
fn a_push_leaves_the_pusher_a_pull_behind_the_server() {
    let mut h = Harness::new(1);
    h.login_all();

    let scheme = h.add_scheme(D0, "Notes", &["first line"]);
    h.sync(D0);

    let document = h.device(D0).workspace.scheme_sync[&scheme].id;
    let (server_seq, _) = h
        .server_document_head(document)
        .expect("server has the scheme document after the first sync");
    let cursor = h.device(D0).local_state().document_cursors[&document].last_pulled_sequence;

    assert!(
        cursor < server_seq,
        "expected the pusher's cursor ({cursor}) to lag the server head ({server_seq}): \
         that gap is the self-echo the next pull returns"
    );
}

/// The echo really does come back down: a second sync with nothing else going
/// on still costs a pull that returns the device's own document.
#[test]
fn the_next_sync_pulls_the_devices_own_document_back() {
    let mut h = Harness::new(1);
    h.login_all();

    let scheme = h.add_scheme(D0, "Notes", &["first line"]);
    h.sync(D0);

    let document = h.device(D0).workspace.scheme_sync[&scheme].id;
    let (server_seq, _) = h.server_document_head(document).expect("document on server");

    // Nothing changed anywhere; this sync exists only to absorb the echo.
    h.sync(D0);

    let cursor = h.device(D0).local_state().document_cursors[&document].last_pulled_sequence;
    assert_eq!(
        cursor, server_seq,
        "the follow-up sync should have pulled the device's own document back, \
         leaving the cursor level with the server head"
    );
}
