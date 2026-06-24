//! Regression tests for signing out of one account and into another.
//!
//! The persisted `sync-state.json` is a single, account-agnostic file. Before the
//! fix, an account switch overwrote only the workspace id / replica id / server url
//! and reused the previous account's per-document pull/push cursors and media
//! cursors. A carried-over cursor is unsafe two ways, both reproduced here:
//!
//!   1. **`crdt_schema_invalid` on push** — a non-zero pull cursor makes the
//!      bootstrap treat a document the new server has no base for as already-present,
//!      so it pushes a bare incremental delta instead of a full snapshot. The server
//!      reconstructs it from empty, finds no `schema` root, and rejects the batch.
//!   2. **Silent data loss / divergence on pull** — the pull is keyed by the stale
//!      cursor, so the new account's documents are not re-pulled from zero.
//!
//! The fix (`LocalSyncState::reset_for_account_change`, wired into desktop
//! `configure_local_state` and mobile `sync_once`) clears those cursors on a detected
//! account/server change so the next sync re-pulls from zero and re-seeds full
//! snapshots, which Yjs merges idempotently. The harness models this in
//! `TestDevice::switch_account`; `switch_account_without_cursor_reset` models the
//! pre-fix driver so the bug itself stays covered.

mod common;

use common::{Harness, TestDevice, TestServer, D0, D1};
use knotq_model::{Workspace, WorkspaceId};

/// Build a fresh device already signed into `account` (no prior content/cursors).
fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    TestDevice::new_from_base(&base, account)
}

fn scheme_item_count(device: &TestDevice, name: &str) -> Option<usize> {
    device
        .workspace
        .schemes
        .values()
        .find(|scheme| scheme.name == name)
        .map(|scheme| scheme.items.len())
}

// ---------------------------------------------------------------------------
// crdt_schema_invalid: the user-reported error
// ---------------------------------------------------------------------------

/// With the fix, signing into a different account re-seeds full snapshots for every
/// document, so the new server never receives a baseless bare delta and never rejects
/// the push with `crdt_schema_invalid`. The full content lands on the new account.
///
/// Setup uses two devices so the switching device has a *non-zero pull cursor* for the
/// scheme (it pulled the scheme rather than authoring it) — the precondition that
/// makes a carried-over cursor dangerous.
#[test]
fn account_switch_reseeds_full_snapshots_without_schema_invalid() {
    let mut h = Harness::new(2);
    h.login_all();

    // D1 authors the scheme; D0 PULLS it, giving D0 a non-zero pull cursor for it.
    let scheme = h.add_scheme(D1, "Plan", &["a"]);
    h.sync(D1);
    h.sync(D0);
    assert!(h.device(D0).has_scheme_named("Plan"));

    // D0 edits the pulled scheme, queuing a bare incremental delta.
    h.append_line(D0, scheme, "b");
    assert!(!h.device(D0).is_fully_pushed());

    // D0 signs out of A and into a fresh account B.
    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();
    {
        let dev = h.device_mut_for_surgery(D0);
        dev.switch_account(account_b, "memory://account-b");
        dev.try_sync(&server_b)
            .expect("sync into fresh account B must succeed");
    }

    assert_eq!(
        server_b.schema_invalid_rejections(),
        0,
        "account switch must re-seed full snapshots, not push baseless bare deltas"
    );
    assert!(
        h.device(D0).is_fully_pushed(),
        "all edits pushed to the new account"
    );

    // A brand-new device on B sees the full migrated content (both lines).
    let mut puller = fresh_device(account_b);
    puller.try_sync(&server_b).expect("puller sync from B");
    assert_eq!(
        scheme_item_count(&puller, "Plan"),
        Some(2),
        "account B must hold the migrated scheme with both lines"
    );
}

/// Switching into a FRESH account re-seeds full snapshots from the server-authoritative
/// bootstrap, so even without an explicit cursor reset no baseless bare delta is pushed
/// (no `crdt_schema_invalid`) and the content still lands. (The cursor reset additionally
/// protects switches into an account that already holds divergent content.)
#[test]
fn account_switch_to_fresh_account_reseeds_without_schema_invalid() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D1, "Plan", &["a"]);
    h.sync(D1);
    h.sync(D0);
    h.append_line(D0, scheme, "b");

    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();
    {
        let dev = h.device_mut_for_surgery(D0);
        dev.switch_account_without_cursor_reset(account_b, "memory://account-b");
        dev.try_sync(&server_b).expect("sync to fresh account B");
    }

    assert_eq!(
        server_b.schema_invalid_rejections(),
        0,
        "fresh-account switch re-seeds full snapshots; no baseless bare delta"
    );
    let mut puller = fresh_device(account_b);
    puller.try_sync(&server_b).expect("puller sync from B");
    assert_eq!(
        scheme_item_count(&puller, "Plan"),
        Some(2),
        "content still carried to the new account"
    );
}

// ---------------------------------------------------------------------------
// Cursor reset + content migration
// ---------------------------------------------------------------------------

/// Switching accounts clears the previous account's cursors immediately, then a sync
/// re-populates cursors for and carries every local document to the new account.
#[test]
fn account_switch_resets_cursors_and_carries_all_content() {
    let mut h = Harness::new(1);
    h.login_all();
    h.add_scheme(D0, "First", &["a"]);
    h.add_scheme(D0, "Second", &["x"]);
    h.sync(D0);
    assert!(
        h.device(D0).document_cursor_count() >= 2,
        "expected cursors for the workspace + schemes on account A"
    );

    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();
    {
        let dev = h.device_mut_for_surgery(D0);
        dev.switch_account(account_b, "memory://account-b");
        // The reset is immediate, before any sync.
        assert_eq!(
            dev.document_cursor_count(),
            0,
            "account switch must clear stale cursors up front"
        );
        dev.try_sync(&server_b).expect("sync into B");
    }

    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert!(h.device(D0).is_fully_pushed());
    assert!(
        h.device(D0).document_cursor_count() >= 2,
        "cursors are re-established against the new account after sync"
    );

    let mut puller = fresh_device(account_b);
    puller.try_sync(&server_b).expect("puller sync");
    assert!(puller.has_scheme_named("First"));
    assert!(puller.has_scheme_named("Second"));
}

/// Signing into an account that ALREADY has content (synced by another device) unions
/// the local and remote workspaces — the device keeps its own schemes and gains the
/// account's existing ones, with no `crdt_schema_invalid`.
#[test]
fn account_switch_unions_local_and_existing_remote_content() {
    // Account B already holds a scheme synced by a different device.
    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();
    {
        let mut prior_b = fresh_device(account_b);
        prior_b.add_scheme("OnlyOnB", &["from-b"]);
        prior_b.try_sync(&server_b).expect("seed account B");
    }

    // A device on account A with its own, different scheme.
    let mut h = Harness::new(1);
    h.login_all();
    h.add_scheme(D0, "OnlyOnA", &["from-a"]);
    h.sync(D0);

    // D0 signs into account B.
    {
        let dev = h.device_mut_for_surgery(D0);
        dev.switch_account(account_b, "memory://account-b");
        dev.try_sync(&server_b).expect("sync into B");
    }

    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert!(
        h.device(D0).has_scheme_named("OnlyOnA"),
        "the device keeps its own content after signing into B"
    );
    assert!(
        h.device(D0).has_scheme_named("OnlyOnB"),
        "the device gains account B's pre-existing content (union merge, no silent skip)"
    );
}

/// Repeatedly switching accounts (A -> B -> A) converges with no rejections and no
/// content loss. Uses standalone servers so each account's reject counter is checked.
#[test]
fn account_switch_round_trip_a_b_a_converges_without_rejections() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();

    let mut dev = fresh_device(account_a);
    dev.add_scheme("Plan", &["a"]);
    dev.try_sync(&server_a).expect("sync to A");

    dev.switch_account(account_b, "memory://account-b");
    dev.try_sync(&server_b).expect("sync to B");
    assert!(dev.has_scheme_named("Plan"), "content survives A -> B");

    dev.switch_account(account_a, "memory://account-a");
    dev.try_sync(&server_a).expect("sync back to A");
    assert!(dev.has_scheme_named("Plan"), "content survives B -> A");

    assert_eq!(server_a.schema_invalid_rejections(), 0);
    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert!(dev.is_fully_pushed());
}
