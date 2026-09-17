//! A new install joining an existing account must never delete that account's
//! data.
//!
//! A real first launch is not an empty workspace: the desktop seeds its starter
//! workspace, and the CRDT documents are unseeded until the first sync. Signing
//! that install into an account that already has content is the single most
//! common way a second device appears — and the production-path fuzzer caught
//! its first sync removing existing schemes and Daily Queue days from the
//! server, after which every device pulled the damaged index.
//!
//! The shared fuzzers never reached this: their fresh devices start from an
//! empty `Workspace::new()`, which takes a different branch.

mod common;

use chrono::NaiveDate;
use common::{TestDevice, TestServer};
use knotq_model::{Item, NodeRef, Scheme, Workspace, WorkspaceId};

fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    TestDevice::new_from_base(&base, account)
}

/// A first launch: local content in the plain workspace, no CRDT history.
fn install_with_local_content(account: WorkspaceId, scheme_name: &str) -> TestDevice {
    let mut base = Workspace::new();
    let mut scheme = Scheme::new(scheme_name, 0);
    scheme.items.push(Item::new("starter line"));
    let scheme_id = scheme.id;
    base.folders
        .get_mut(&base.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    base.schemes.insert(scheme_id, scheme);
    base.ensure_sync_metadata();
    TestDevice::new_from_base(&base, account)
}

#[test]
fn a_new_install_signing_in_keeps_the_accounts_existing_content() {
    let account = WorkspaceId::new();
    let server = TestServer::default();

    let mut existing = fresh_device(account);
    existing.add_scheme("Existing plans", &["keep me"]);
    let folder = existing.add_folder("Existing folder");
    existing.add_scheme_to_folder(folder, "Filed plans", &["also keep me"]);
    existing.set_daily_queue(NaiveDate::from_ymd_opt(2026, 9, 15).unwrap(), &["a day"]);
    existing.try_sync(&server).unwrap();

    let mut joining = install_with_local_content(account, "Starter");
    for _ in 0..3 {
        joining.try_sync(&server).unwrap();
        existing.try_sync(&server).unwrap();
    }

    let mut puller = fresh_device(account);
    for _ in 0..3 {
        puller.try_sync(&server).unwrap();
    }
    for device in [&existing, &joining, &puller] {
        for name in ["Existing plans", "Filed plans"] {
            assert!(
                device.has_scheme_named(name),
                "joining an account lost scheme {name:?}"
            );
        }
        assert!(
            device.has_folder_named("Existing folder"),
            "joining an account lost a folder"
        );
        assert!(
            device
                .workspace
                .daily_queue
                .contains_key(&NaiveDate::from_ymd_opt(2026, 9, 15).unwrap()),
            "joining an account lost a Daily Queue day"
        );
    }
    assert!(existing.converges_with(&joining));
    assert!(existing.converges_with(&puller));
}
