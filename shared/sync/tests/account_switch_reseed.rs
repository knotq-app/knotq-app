//! Regression tests for the account-switch push corruption.
//!
//! Scheme and daily-queue content documents are keyed by ids *derived* from the
//! scheme id / date, so the same document id exists on every account. A device
//! that switches accounts therefore meets a server holding an unrelated base
//! under an id its own document also uses. Pushing an incremental
//! `encode_diff_v1` there — a delta that assumes the receiver already has this
//! device's history — could delete structs the base never had (observed: the
//! scheme's `schema` key), which the server rejects as `crdt_schema_invalid`.
//! The device then re-queued the same delta forever, wedging the push loop.
//!
//! Found by the property fuzzer only at ~10x its default depth
//! (`account_hopping_fuzz_converges`, 1500 seeds x 400 steps), so these tests
//! pin the specific behaviour directly rather than relying on a deep fuzz run
//! to stumble on it again.

mod common;

use chrono::NaiveDate;
use common::{TestDevice, TestServer};
use knotq_model::{Workspace, WorkspaceId};

const SERVER_A: &str = "memory://account-a";
const SERVER_B: &str = "memory://account-b";

fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    TestDevice::new_from_base(&base, account)
}

fn settle(device: &mut TestDevice, server: &TestServer) {
    for _ in 0..8 {
        let _ = device.try_sync(server);
    }
}

/// The core regression: a device carrying content for a *derived* document id
/// switches to an account whose server already holds a base for that same id.
/// The push must be accepted — never rejected as `crdt_schema_invalid`.
#[test]
fn switching_into_an_account_that_already_holds_the_document_never_pushes_a_corrupt_delta() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();
    let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

    // Another device establishes a base for this date's daily-queue document on
    // account B, so the id is already occupied there with foreign history.
    let mut resident = fresh_device(account_b);
    resident.set_daily_queue(date, &["resident line"]);
    settle(&mut resident, &server_b);
    assert!(server_b.document_count() > 0, "account B holds a base");

    // Our device builds its own history for the SAME derived id on account A.
    let mut roamer = fresh_device(account_a);
    let scheme = roamer.set_daily_queue(date, &["roamer line"]);
    settle(&mut roamer, &server_a);
    roamer.append_line(scheme, "roamer second line");
    settle(&mut roamer, &server_a);

    // Now switch it to account B and sync.
    roamer.switch_account(account_b, SERVER_B);
    settle(&mut roamer, &server_b);

    assert_eq!(
        server_b.schema_invalid_rejections(),
        0,
        "a device that switched accounts pushed a delta the new server's base \
         could not accept"
    );
    assert!(
        roamer.is_fully_pushed(),
        "the push loop wedged behind a rejected delta"
    );
}

/// The same hazard when only the *server* changes (prod -> sandbox) under one
/// account id: different backend, different history, same derived document ids.
#[test]
fn switching_server_under_the_same_account_never_pushes_a_corrupt_delta() {
    let account = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();
    let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

    let mut resident = fresh_device(account);
    resident.set_daily_queue(date, &["resident line"]);
    settle(&mut resident, &server_b);

    let mut roamer = fresh_device(account);
    let scheme = roamer.set_daily_queue(date, &["roamer line"]);
    settle(&mut roamer, &server_a);
    roamer.append_line(scheme, "more");
    settle(&mut roamer, &server_a);

    roamer.switch_account(account, SERVER_B);
    settle(&mut roamer, &server_b);

    assert_eq!(server_b.schema_invalid_rejections(), 0);
    assert!(roamer.is_fully_pushed());
}

/// Hopping back and forth repeatedly is what the fuzzer does; each arrival must
/// re-arm the re-seed, not just the first.
#[test]
fn repeated_account_hops_never_push_a_corrupt_delta() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();
    let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

    let mut other_a = fresh_device(account_a);
    other_a.set_daily_queue(date, &["a-resident"]);
    settle(&mut other_a, &server_a);
    let mut other_b = fresh_device(account_b);
    other_b.set_daily_queue(date, &["b-resident"]);
    settle(&mut other_b, &server_b);

    let mut roamer = fresh_device(account_a);
    let scheme = roamer.set_daily_queue(date, &["roamer"]);
    settle(&mut roamer, &server_a);

    for hop in 0..6 {
        let (account, url, server) = if hop % 2 == 0 {
            (account_b, SERVER_B, &server_b)
        } else {
            (account_a, SERVER_A, &server_a)
        };
        roamer.switch_account(account, url);
        roamer.append_line(scheme, &format!("hop {hop}"));
        settle(&mut roamer, server);
        assert_eq!(
            server_a.schema_invalid_rejections() + server_b.schema_invalid_rejections(),
            0,
            "hop {hop} pushed a delta the destination could not accept"
        );
        assert!(roamer.is_fully_pushed(), "hop {hop} wedged the push loop");
    }
}

/// The content the device carried over must actually arrive — the re-seed is a
/// full snapshot precisely so nothing is dropped in exchange for correctness.
#[test]
fn a_switched_device_delivers_its_content_to_the_new_account() {
    let account_a = WorkspaceId::new();
    let account_b = WorkspaceId::new();
    let server_a = TestServer::default();
    let server_b = TestServer::default();
    let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

    let mut resident = fresh_device(account_b);
    resident.set_daily_queue(date, &["resident line"]);
    settle(&mut resident, &server_b);

    let mut roamer = fresh_device(account_a);
    let scheme = roamer.set_daily_queue(date, &["carried line"]);
    settle(&mut roamer, &server_a);

    roamer.switch_account(account_b, SERVER_B);
    settle(&mut roamer, &server_b);
    settle(&mut resident, &server_b);

    assert_eq!(server_b.schema_invalid_rejections(), 0);
    // Both devices on account B must agree, and a fresh puller must see the same
    // thing — i.e. the server really holds the union, not one side's view.
    assert!(
        roamer.converges_with(&resident),
        "the switched device and the resident device diverged on account B"
    );
    let mut puller = fresh_device(account_b);
    settle(&mut puller, &server_b);
    assert!(
        puller.converges_with(&roamer),
        "a fresh device does not see what the switched device delivered"
    );
    let _ = scheme;
}
