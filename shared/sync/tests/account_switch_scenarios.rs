//! Exhaustive sign-out / sign-in (account switch) sync scenarios.
//!
//! Goal: sync must work *no matter what* across account switches. Each test signs a
//! device out of one account and into another (different workspace id and/or server)
//! and asserts the new account ends up with the correct content, the device converges,
//! and the backend never rejects a push with `crdt_schema_invalid`
//! (`TestServer::schema_invalid_rejections() == 0`).
//!
//! The harness models a real account switch with `TestDevice::switch_account` (adopt
//! the new canonical workspace identity, re-key the workspace CRDT doc, reset stale
//! cursors). `switch_account_without_cursor_reset` models the pre-fix driver so the
//! bugs the reset fixes stay covered. See `account_switch_regressions.rs` for the
//! focused `crdt_schema_invalid` regression pair.

mod common;

use chrono::NaiveDate;
use common::{Harness, TestDevice, TestServer, D0, D1};
use knotq_model::{Workspace, WorkspaceId};

/// A brand-new device freshly signed into `account` (no prior content/cursors).
fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    TestDevice::new_from_base(&base, account)
}

/// Pull everything `server` holds into a brand-new device on `account`.
fn puller_for(account: WorkspaceId, server: &TestServer) -> TestDevice {
    let mut puller = fresh_device(account);
    puller.try_sync(server).expect("puller sync");
    puller
}

// ---------------------------------------------------------------------------
// Content fidelity: folders, archive, daily queue, media, scale
// ---------------------------------------------------------------------------

#[test]
fn switch_carries_folders_and_nested_schemes() {
    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();

    let mut h = Harness::new(1);
    h.login_all();
    let work = h.add_folder(D0, "Work");
    let sub = h.add_subfolder(D0, work, "Projects");
    h.add_scheme_to_folder(D0, work, "WorkTop", &["w"]);
    h.add_scheme_to_folder(D0, sub, "DeepScheme", &["d"]);
    h.add_scheme(D0, "RootScheme", &["r"]);
    h.sync(D0);

    {
        let dev = h.device_mut_for_surgery(D0);
        dev.switch_account(account_b, "memory://account-b");
        dev.try_sync(&server_b).expect("sync into B");
    }
    assert_eq!(server_b.schema_invalid_rejections(), 0);

    let puller = puller_for(account_b, &server_b);
    assert!(puller.has_folder_named("Work"));
    assert!(puller.has_folder_named("Projects"));
    assert!(puller.has_scheme_named("WorkTop"));
    assert!(puller.has_scheme_named("DeepScheme"));
    assert!(puller.has_scheme_named("RootScheme"));
}

#[test]
fn switch_carries_archived_scheme_preserving_archive_state() {
    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();

    let mut h = Harness::new(1);
    h.login_all();
    h.add_scheme(D0, "Active", &["a"]);
    let archived = h.add_scheme(D0, "Archived", &["old"]);
    h.archive_scheme(D0, archived);
    h.sync(D0);

    {
        let dev = h.device_mut_for_surgery(D0);
        dev.switch_account(account_b, "memory://account-b");
        dev.try_sync(&server_b).expect("sync into B");
    }
    assert_eq!(server_b.schema_invalid_rejections(), 0);

    let puller = puller_for(account_b, &server_b);
    assert!(puller.has_scheme_named("Active"));
    assert!(
        puller.workspace.schemes.contains_key(&archived),
        "archived scheme data survives the switch"
    );
    assert!(
        puller.scheme_is_archived(archived),
        "archive (recently_deleted) state survives the switch"
    );
}

#[test]
fn switch_carries_daily_queue() {
    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();
    let date = NaiveDate::from_ymd_opt(2026, 6, 23).unwrap();

    let mut h = Harness::new(1);
    h.login_all();
    let dq = h.device_mut_for_surgery(D0).set_daily_queue(date, &["plan the day"]);
    h.sync(D0);

    {
        let dev = h.device_mut_for_surgery(D0);
        dev.switch_account(account_b, "memory://account-b");
        dev.try_sync(&server_b).expect("sync into B");
    }
    assert_eq!(server_b.schema_invalid_rejections(), 0);

    let puller = puller_for(account_b, &server_b);
    assert_eq!(
        puller.workspace.daily_queue.get(&date).copied(),
        Some(dq),
        "daily-queue mapping carried to the new account"
    );
    assert!(puller.workspace.schemes.contains_key(&dq));
}

#[test]
fn switch_carries_media_to_new_account() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();

    let mut dev = fresh_device(account_a);
    let scheme = dev.add_scheme("Photos", &["caption"]);
    let bytes = vec![9u8, 8, 7, 6, 5];
    let (_asset, image_name) = dev.attach_image(scheme, 0, bytes.clone());
    dev.try_sync(&server_a).expect("crdt sync to A");
    let remote_a = dev.remote_latest_after_sync();
    dev.upload_media_to(&server_a, &remote_a).expect("upload media to A");
    assert_eq!(server_a.media_asset_count(), 1);

    dev.switch_account(account_b, "memory://account-b");
    assert_eq!(dev.media_cursor_count(), 0, "switch clears media cursors");
    dev.try_sync(&server_b).expect("crdt sync to B");
    let remote_b = dev.remote_latest_after_sync();
    dev.upload_media_to(&server_b, &remote_b).expect("upload media to B");
    assert_eq!(
        server_b.media_asset_count(),
        1,
        "media re-uploaded to the new account after the switch"
    );

    let mut puller = puller_for(account_b, &server_b);
    puller.download_media_from(&server_b);
    assert_eq!(
        puller.media_assets.get(&image_name),
        Some(&bytes),
        "a fresh device on B downloads the migrated image"
    );
}

#[test]
fn switch_resets_media_cursors() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();

    let mut dev = fresh_device(account_a);
    let scheme = dev.add_scheme("Photos", &["caption"]);
    dev.attach_image(scheme, 0, vec![1, 2, 3, 4]);
    dev.try_sync(&server_a).expect("crdt sync to A");
    let remote_a = dev.remote_latest_after_sync();
    dev.upload_media_to(&server_a, &remote_a).expect("upload to A");
    assert_eq!(dev.media_cursor_count(), 1);

    dev.switch_account(account_b, "memory://account-b");
    assert_eq!(
        dev.media_cursor_count(),
        0,
        "account switch must clear media cursors so media re-uploads to the new account"
    );
}

#[test]
fn switch_with_many_schemes_no_schema_invalid() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();

    let mut dev = fresh_device(account_a);
    for i in 0..25 {
        dev.add_scheme(&format!("Scheme{i}"), &["x"]);
    }
    dev.try_sync(&server_a).expect("sync to A");

    dev.switch_account(account_b, "memory://account-b");
    dev.try_sync(&server_b).expect("sync to B");
    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert!(dev.is_fully_pushed());

    let puller = puller_for(account_b, &server_b);
    let migrated = puller
        .workspace
        .schemes
        .values()
        .filter(|s| s.name.starts_with("Scheme"))
        .count();
    assert_eq!(migrated, 25, "all 25 schemes migrated to the new account");
}

#[test]
fn switch_flushes_multiple_unsynced_pending_edits() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();

    let mut dev = fresh_device(account_a);
    let alpha = dev.add_scheme("Alpha", &["1"]);
    let beta = dev.add_scheme("Beta", &["1"]);
    let gamma = dev.add_scheme("Gamma", &["1"]);
    dev.try_sync(&server_a).expect("sync to A");

    // Edits made offline, never synced to A, must still reach B after the switch.
    dev.append_line(alpha, "2");
    dev.append_line(beta, "2");
    dev.append_line(gamma, "2");
    assert!(!dev.is_fully_pushed());

    dev.switch_account(account_b, "memory://account-b");
    dev.try_sync(&server_b).expect("sync to B");
    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert!(dev.is_fully_pushed());

    let puller = puller_for(account_b, &server_b);
    assert_eq!(puller.scheme_line_count("Alpha"), Some(2));
    assert_eq!(puller.scheme_line_count("Beta"), Some(2));
    assert_eq!(puller.scheme_line_count("Gamma"), Some(2));
}

// ---------------------------------------------------------------------------
// Same-account relogin must NOT over-reset
// ---------------------------------------------------------------------------

#[test]
fn relogin_same_account_keeps_cursors_and_converges() {
    let account = WorkspaceId::new();
    let server = TestServer::default();

    let mut dev = fresh_device(account);
    dev.add_scheme("Plan", &["a"]);
    dev.switch_account(account, "memory://prod"); // establish server url (no reset)
    dev.try_sync(&server).expect("first sync");
    let cursors = dev.document_cursor_count();
    assert!(cursors >= 2);

    // Sign out and back into the SAME account + server.
    dev.switch_account(account, "memory://prod");
    assert_eq!(
        dev.document_cursor_count(),
        cursors,
        "re-login to the same account must NOT discard cursors (no needless full re-pull)"
    );

    dev.try_sync(&server).expect("second sync");
    assert!(dev.is_fully_pushed());
    assert_eq!(server.schema_invalid_rejections(), 0);
}

// ---------------------------------------------------------------------------
// Server change with the same workspace id (prod -> sandbox)
// ---------------------------------------------------------------------------

#[test]
fn switch_server_same_workspace_id_resets_cursors() {
    let account = WorkspaceId::new();
    let server_prod = TestServer::default();
    let server_sandbox = TestServer::default();

    let mut dev = fresh_device(account);
    dev.add_scheme("Plan", &["a"]);
    dev.switch_account(account, "memory://prod"); // establish prod url (no reset)
    dev.try_sync(&server_prod).expect("sync prod");
    assert!(dev.document_cursor_count() >= 2);

    // Same workspace id, different backend.
    dev.switch_account(account, "memory://sandbox");
    assert_eq!(
        dev.document_cursor_count(),
        0,
        "a server change (prod -> sandbox) must reset cursors even when the workspace id is unchanged"
    );
    dev.try_sync(&server_sandbox).expect("sync sandbox");
    assert_eq!(server_sandbox.schema_invalid_rejections(), 0);

    let puller = puller_for(account, &server_sandbox);
    assert!(puller.has_scheme_named("Plan"));
}

// ---------------------------------------------------------------------------
// Multi-account chains, multi-device, repeated switching
// ---------------------------------------------------------------------------

#[test]
fn switch_chain_a_b_c_no_cross_contamination() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let account_c = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();
    let server_c = TestServer::default();

    let mut dev = fresh_device(account_a);
    dev.add_scheme("Plan", &["a"]);
    dev.try_sync(&server_a).expect("sync A");

    dev.switch_account(account_b, "memory://b");
    dev.try_sync(&server_b).expect("sync B");

    dev.switch_account(account_c, "memory://c");
    dev.try_sync(&server_c).expect("sync C");

    assert_eq!(server_a.schema_invalid_rejections(), 0);
    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert_eq!(server_c.schema_invalid_rejections(), 0);

    // Each account ends up holding the content (carried forward through the chain).
    assert!(puller_for(account_a, &server_a).has_scheme_named("Plan"));
    assert!(puller_for(account_b, &server_b).has_scheme_named("Plan"));
    assert!(puller_for(account_c, &server_c).has_scheme_named("Plan"));
}

#[test]
fn migrated_account_converges_with_second_device() {
    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();

    // D0 starts on account A, then migrates into account B.
    let mut h = Harness::new(1);
    h.login_all();
    h.add_scheme(D0, "FromA", &["a"]);
    h.sync(D0);
    {
        let dev = h.device_mut_for_surgery(D0);
        dev.switch_account(account_b, "memory://account-b");
        dev.try_sync(&server_b).expect("migrate to B");
    }

    // A second, brand-new device on account B adds its own scheme.
    let mut second = fresh_device(account_b);
    second.add_scheme("FromSecond", &["s"]);
    second.try_sync(&server_b).expect("second device sync");

    // Converge both devices on account B.
    for _ in 0..3 {
        h.device_mut_for_surgery(D0)
            .try_sync(&server_b)
            .expect("D0 converge");
        second.try_sync(&server_b).expect("second converge");
    }

    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert!(h.device(D0).has_scheme_named("FromA"));
    assert!(h.device(D0).has_scheme_named("FromSecond"));
    assert!(second.has_scheme_named("FromA"));
    assert!(second.has_scheme_named("FromSecond"));
}

#[test]
fn switch_back_and_forth_repeatedly_stays_consistent() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();

    let mut dev = fresh_device(account_a);
    dev.add_scheme("Plan", &["a"]);
    dev.switch_account(account_a, "memory://a"); // establish url
    dev.try_sync(&server_a).expect("sync A");

    for _ in 0..3 {
        dev.switch_account(account_b, "memory://b");
        dev.try_sync(&server_b).expect("sync B");
        assert!(dev.has_scheme_named("Plan"), "content survives A -> B");

        dev.switch_account(account_a, "memory://a");
        dev.try_sync(&server_a).expect("sync A");
        assert!(dev.has_scheme_named("Plan"), "content survives B -> A");
    }

    assert_eq!(server_a.schema_invalid_rejections(), 0);
    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert!(dev.is_fully_pushed());
}

#[test]
fn switch_then_double_sync_is_idempotent() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();

    let mut dev = fresh_device(account_a);
    dev.add_scheme("Plan", &["a"]);
    dev.try_sync(&server_a).expect("sync A");

    dev.switch_account(account_b, "memory://b");
    dev.try_sync(&server_b).expect("first sync to B");
    assert!(dev.is_fully_pushed());

    let docs_after_first = server_b.document_count();
    dev.try_sync(&server_b).expect("second sync to B");
    assert!(dev.is_fully_pushed(), "no new pending after a redundant sync");
    assert_eq!(
        server_b.document_count(),
        docs_after_first,
        "a redundant sync must not create new documents"
    );
    assert_eq!(server_b.schema_invalid_rejections(), 0);
}

#[test]
fn switch_into_empty_account_seeds_it_fully() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();

    let mut dev = fresh_device(account_a);
    dev.add_scheme("One", &["1"]);
    dev.add_scheme("Two", &["2"]);
    dev.try_sync(&server_a).expect("sync A");

    dev.switch_account(account_b, "memory://b");
    dev.try_sync(&server_b).expect("seed empty B");
    assert_eq!(server_b.schema_invalid_rejections(), 0);

    let puller = puller_for(account_b, &server_b);
    assert!(puller.has_scheme_named("One"));
    assert!(puller.has_scheme_named("Two"));
}

// ---------------------------------------------------------------------------
// Silent-skip bug (pull cursor) — demonstrated and fixed
// ---------------------------------------------------------------------------

/// A device that PULLED a scheme on account A (non-zero pull cursor, no pending) signs
/// into a FRESH account B. The server-authoritative bootstrap re-seeds full snapshots
/// for documents B lacks, so the scheme is carried even without an explicit cursor
/// reset — no silent loss for the fresh-account case.
#[test]
fn switch_to_fresh_account_carries_pulled_scheme_via_reseed() {
    let mut h = Harness::new(2);
    h.login_all();
    h.add_scheme(D0, "Shared", &["x"]);
    // D0 authors; D1 PULLS it so D1 has a non-zero pull cursor and no pending.
    h.sync(D0);
    h.sync(D1);
    assert!(h.device(D1).has_scheme_named("Shared"));

    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();
    {
        let dev = h.device_mut_for_surgery(D1);
        dev.switch_account_without_cursor_reset(account_b, "memory://account-b");
        dev.try_sync(&server_b).expect("sync to B");
    }
    assert_eq!(server_b.schema_invalid_rejections(), 0);

    let puller = puller_for(account_b, &server_b);
    assert_eq!(
        puller.scheme_line_count("Shared"),
        Some(1),
        "fresh-account re-seed carries the pulled scheme even without an explicit cursor reset"
    );
}

/// With the fix, the same setup carries the pulled scheme's full content to B.
#[test]
fn switch_with_reset_carries_pulled_scheme() {
    let mut h = Harness::new(2);
    h.login_all();
    h.add_scheme(D0, "Shared", &["x"]);
    h.sync(D0);
    h.sync(D1);

    let account_b = WorkspaceId::new();
    let server_b = TestServer::default();
    {
        let dev = h.device_mut_for_surgery(D1);
        dev.switch_account(account_b, "memory://account-b");
        dev.try_sync(&server_b).expect("sync to B");
    }
    assert_eq!(server_b.schema_invalid_rejections(), 0);

    let puller = puller_for(account_b, &server_b);
    assert_eq!(
        puller.scheme_line_count("Shared"),
        Some(1),
        "with the cursor reset the pulled scheme is fully carried to the new account"
    );
}
