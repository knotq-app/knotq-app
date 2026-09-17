//! Two devices changing *different* attributes of the same line at the same
//! time must both keep their change.
//!
//! The production-path fuzzer caught a line's marker family reverting during a
//! sync with no other device ever touching it. Line metadata used to be read
//! back from one whole-item snapshot, so any metadata write from a stale copy
//! restored the fields it did not mean to change. Each field is now merged from
//! its own key (the snapshot is still written for older builds).

mod common;

use common::{TestDevice, TestServer};
use knotq_model::{ItemMarker, MarkerFamily, Workspace, WorkspaceId};

fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    TestDevice::new_from_base(&base, account)
}

fn two_synced_devices() -> (TestServer, TestDevice, TestDevice, knotq_model::SchemeId) {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut a = fresh_device(account);
    let mut b = fresh_device(account);
    let scheme = a.add_scheme("Lists", &["first", "second", "third"]);
    for _ in 0..2 {
        a.try_sync(&server).unwrap();
        b.try_sync(&server).unwrap();
    }
    (server, a, b, scheme)
}

fn settle(server: &TestServer, a: &mut TestDevice, b: &mut TestDevice) {
    for _ in 0..3 {
        a.try_sync(server).unwrap();
        b.try_sync(server).unwrap();
    }
}

#[test]
fn a_marker_family_and_an_indent_change_on_the_same_line_both_survive() {
    let (server, mut a, mut b, scheme) = two_synced_devices();
    a.set_marker_family(scheme, 0, ItemMarker::Bullet, MarkerFamily::Rings);
    b.set_item_indent(scheme, 0, 2);
    settle(&server, &mut a, &mut b);

    for (label, device) in [("a", &a), ("b", &b)] {
        let item = &device.workspace.schemes[&scheme].items[0];
        assert_eq!(
            item.marker_family,
            MarkerFamily::Rings,
            "{label}: family reverted"
        );
        assert_eq!(item.indent, 2, "{label}: indent reverted");
    }
}

#[test]
fn a_reorder_does_not_revert_a_concurrent_marker_change() {
    let (server, mut a, mut b, scheme) = two_synced_devices();
    a.set_item_marker(scheme, 2, ItemMarker::Checkbox);
    b.reorder_reverse(scheme);
    settle(&server, &mut a, &mut b);

    for (label, device) in [("a", &a), ("b", &b)] {
        let third = device.workspace.schemes[&scheme]
            .items
            .iter()
            .find(|item| item.text() == "third")
            .unwrap();
        assert_eq!(
            third.marker,
            ItemMarker::Checkbox,
            "{label}: marker reverted by a reorder"
        );
    }
}

/// Reading a line back must give exactly what was written, even when the
/// combination is one the editor would normalize (a bullet that still carries
/// dates). A normalizing read made every sync see the line as changed and
/// rewrite it; under compaction that churn lost other devices' folder and
/// archive edits on the server (compaction fuzz seeds 114 and 246).
#[test]
fn a_line_reads_back_exactly_as_written_so_syncing_queues_nothing_more() {
    let (server, mut a, mut b, scheme) = two_synced_devices();
    let start = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
    a.set_item_dates(scheme, 1, Some(start), None);
    a.set_item_marker(scheme, 1, ItemMarker::Bullet);
    settle(&server, &mut a, &mut b);

    let written = a.workspace.schemes[&scheme].items[1].clone();
    assert_eq!(written.marker, ItemMarker::Bullet);
    assert_eq!(
        written.start,
        Some(start),
        "a's own line was rewritten by its read-back"
    );
    assert_eq!(
        b.workspace.schemes[&scheme].items[1], written,
        "b sees a different line"
    );

    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();
    assert!(
        a.is_fully_pushed(),
        "a re-queued an unchanged line: {:?}",
        a.pending_edits()
    );
    assert!(
        b.is_fully_pushed(),
        "b re-queued an unchanged line: {:?}",
        b.pending_edits()
    );
}
