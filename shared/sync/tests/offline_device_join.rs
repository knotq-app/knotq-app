//! A device with real offline history joining an account must not delete what
//! the account already holds.
//!
//! Unlike `fresh_install_join.rs`, this device's CRDT documents are seeded: it
//! was used — schemes created, days opened — before its first sync. The
//! production-path fuzzer caught such a device's first sync removing another
//! device's Daily Queue day from the server.

mod common;

use chrono::NaiveDate;
use common::{TestDevice, TestServer};
use knotq_model::{Workspace, WorkspaceId};

const ACCOUNT_URL: &str = "memory://account";

fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    TestDevice::new_from_base(&base, account)
}

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, d).unwrap()
}

/// The account already holds a scheme and a day another device opened.
fn account_with_content(server: &TestServer, account: WorkspaceId) -> TestDevice {
    let mut existing = fresh_device(account);
    existing.add_scheme("Existing plans", &["keep me"]);
    existing.set_daily_queue(day(6), &["a past day"]);
    existing.try_sync(server).unwrap();
    existing
}

fn assert_nothing_lost(devices: &[&TestDevice]) {
    for device in devices {
        assert!(
            device.has_scheme_named("Existing plans"),
            "the account's existing scheme was lost"
        );
        assert!(
            device.workspace.daily_queue.contains_key(&day(6)),
            "the account's existing Daily Queue day was lost"
        );
        assert!(
            device.has_scheme_named("Made offline"),
            "the joining device's own scheme was lost"
        );
        assert!(
            device.workspace.daily_queue.contains_key(&day(14)),
            "the joining device's own day was lost"
        );
    }
}

fn settle(server: &TestServer, devices: &mut [&mut TestDevice]) {
    for _ in 0..4 {
        for device in devices.iter_mut() {
            device.try_sync(server).unwrap();
        }
    }
}

/// Installed and used under its own local identity, then signed in.
#[test]
fn offline_history_then_first_sign_in_keeps_the_accounts_content() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut existing = account_with_content(&server, account);

    let mut joining = fresh_device(WorkspaceId::new());
    joining.add_scheme("Made offline", &["offline line"]);
    joining.set_daily_queue(day(14), &["today, offline"]);
    joining.switch_account(account, ACCOUNT_URL);

    settle(&server, &mut [&mut joining, &mut existing]);
    let mut puller = fresh_device(account);
    settle(&server, &mut [&mut puller]);

    assert_nothing_lost(&[&existing, &joining, &puller]);
    assert!(existing.converges_with(&joining));
    assert!(existing.converges_with(&puller));
}

/// Already bound to the account, but used offline before its first sync.
#[test]
fn offline_history_on_a_bound_device_keeps_the_accounts_content() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut existing = account_with_content(&server, account);

    let mut joining = fresh_device(account);
    joining.add_scheme("Made offline", &["offline line"]);
    joining.set_daily_queue(day(14), &["today, offline"]);

    settle(&server, &mut [&mut joining, &mut existing]);
    let mut puller = fresh_device(account);
    settle(&server, &mut [&mut puller]);

    assert_nothing_lost(&[&existing, &joining, &puller]);
    assert!(existing.converges_with(&joining));
    assert!(existing.converges_with(&puller));
}

/// Used offline under its own local identity by a device that has never synced
/// at all — no workspace id in its local sync state, exactly as a desktop or
/// mobile install's `sync-state.json` looks before its first sign-in — then
/// signed in. Its first sync re-keys the workspace document from the local id
/// to the account's.
#[test]
fn never_synced_offline_history_then_first_sign_in_keeps_the_accounts_content() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut existing = account_with_content(&server, account);

    let mut joining = fresh_device(WorkspaceId::new());
    joining.add_scheme("Made offline", &["offline line"]);
    joining.set_daily_queue(day(14), &["today, offline"]);
    joining.local_state_mut().workspace_id = None;
    joining.local_state_mut().server_url = None;
    joining.switch_account(account, ACCOUNT_URL);

    settle(&server, &mut [&mut joining, &mut existing]);
    let mut puller = fresh_device(account);
    settle(&server, &mut [&mut puller]);

    assert_nothing_lost(&[&existing, &joining, &puller]);
    assert!(existing.converges_with(&joining));
    assert!(existing.converges_with(&puller));
}
