//! Two installs that seed the same starter content — same scheme ids, same
//! item ids, no CRDT history — and sign into one account must end up with that
//! content exactly once.
//!
//! The desktop starter workspace uses fixed ids, so every first launch creates
//! the same lines independently. The production-path fuzzer caught a starter
//! line duplicated inside its scheme on every device and on the server, and
//! never converging back to one.

mod common;

use std::collections::HashMap;

use common::{TestDevice, TestServer};
use knotq_model::{Item, ItemId, NodeRef, Scheme, SchemeId, Workspace, WorkspaceId};

fn fixed_scheme() -> Scheme {
    let mut scheme = Scheme::new("Start here", 0);
    scheme.id = "00000000-0000-8000-8000-000000000101"
        .parse::<SchemeId>()
        .unwrap();
    for (index, text) in ["Thesis", "Argument", "Final draft"]
        .into_iter()
        .enumerate()
    {
        let mut item = Item::new(text);
        item.id = format!("00000000-0000-8000-8000-00000000100{}", index + 3)
            .parse::<ItemId>()
            .unwrap();
        scheme.items.push(item);
    }
    scheme
}

/// A first launch: the starter scheme in the plain workspace, nothing in CRDT.
fn install(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    let scheme = fixed_scheme();
    let id = scheme.id;
    base.folders
        .get_mut(&base.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(id));
    base.schemes.insert(id, scheme);
    base.ensure_sync_metadata();
    TestDevice::new_from_base(&base, account)
}

fn assert_each_item_once(label: &str, device: &TestDevice) {
    let mut counts: HashMap<ItemId, usize> = HashMap::new();
    for scheme in device.workspace.schemes.values() {
        for item in &scheme.items {
            *counts.entry(item.id).or_default() += 1;
        }
    }
    let duplicated: Vec<_> = counts.iter().filter(|(_, count)| **count > 1).collect();
    assert!(
        duplicated.is_empty(),
        "{label}: starter items duplicated: {duplicated:?}"
    );
}

#[test]
fn two_installs_with_the_same_starter_content_converge_without_duplicates() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut first = install(account);
    let mut second = install(account);

    for _ in 0..4 {
        first.try_sync(&server).unwrap();
        second.try_sync(&server).unwrap();
    }
    assert_each_item_once("first install", &first);
    assert_each_item_once("second install", &second);
    assert!(first.converges_with(&second));
}

#[test]
fn installs_that_both_edit_before_syncing_converge_without_duplicates() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut first = install(account);
    let mut second = install(account);
    let scheme = fixed_scheme().id;
    first.append_line(scheme, "first device's line");
    second.edit_line(scheme, 0, "second device's thesis");

    for _ in 0..4 {
        first.try_sync(&server).unwrap();
        second.try_sync(&server).unwrap();
    }
    assert_each_item_once("first install", &first);
    assert_each_item_once("second install", &second);
    assert!(first.converges_with(&second));
}

/// The account already holds the starter lines when a second install, which
/// has never synced, deletes one of them and retypes another. Its first edit
/// used to write the whole scheme as brand-new content: the delete could not be
/// expressed (the deleted line came back from the account) and every line's
/// text was inserted a second time into the shared line.
#[test]
fn a_starter_line_deleted_or_retyped_before_joining_lands_once() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut first = install(account);
    for _ in 0..2 {
        first.try_sync(&server).unwrap();
    }

    let mut second = install(account);
    let scheme = fixed_scheme().id;
    let deleted = fixed_scheme().items[0].id;
    let retyped = fixed_scheme().items[1].id;
    second.remove_line(scheme, 0);
    second.edit_line(scheme, 0, "retyped before joining");

    for _ in 0..4 {
        second.try_sync(&server).unwrap();
        first.try_sync(&server).unwrap();
    }
    for (label, device) in [("first install", &first), ("second install", &second)] {
        assert_each_item_once(label, device);
        let items = &device.workspace.schemes[&scheme].items;
        assert!(
            items.iter().all(|item| item.id != deleted),
            "{label}: the deleted starter line came back"
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == retyped)
                .map(|item| item.text()),
            Some("retyped before joining".to_string()),
            "{label}: the retyped starter line"
        );
        let untouched = items
            .iter()
            .find(|item| item.id == fixed_scheme().items[2].id)
            .map(|item| item.text());
        assert_eq!(
            untouched,
            Some("Final draft".to_string()),
            "{label}: an untouched starter line's text was repeated"
        );
    }
    assert!(first.converges_with(&second));
}
