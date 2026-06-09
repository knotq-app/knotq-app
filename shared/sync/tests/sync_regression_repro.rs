//! Reproductions of the user-reported sync correctness failures, driven through the
//! *faithful* harness (`common`) that now rebuilds each device's store CRDT from the
//! materialized workspace after every sync — exactly like the desktop
//! (`WorkspaceStore::replace_workspace`) and mobile (`sync_once`) drivers. That
//! rebuild mints a fresh Yjs clientID and re-encodes all content from clock 0, so a
//! later local diff references a causal base the server never received and the edit
//! is silently dropped on merge.
//!
//! These tests are the regression net for persisted CRDT documents with a stable
//! replica identity and for archiving folders as structured, restorable units.

mod common;

use common::{Harness, D0, D1};

/// Bug #1: a rename does not propagate even though both devices report "synced".
///
/// After the first settle every device has rebuilt its store CRDT, so D0's rename
/// delta is encoded against a base (its freshly re-encoded workspace doc) that the
/// server never saw. The delta buffers unintegrated on merge; the rename is lost and
/// both devices converge back to the old name.
#[test]
fn rename_propagates_between_devices() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Plan", &["body"]);
    h.settle();
    h.assert_all_converged();

    h.rename_scheme(D0, scheme, "Final");
    h.settle();

    h.assert_all_converged();
    h.assert_scheme_name(D0, scheme, "Final");
    h.assert_scheme_name(D1, scheme, "Final");
}

/// Bug #2: a scheme created on one device and then edited on another loses the edit
/// (and, in the worst interleavings, vanishes), because the cross-device content
/// edit is diffed against the editing device's rebuilt scheme doc rather than the
/// state the server holds.
#[test]
fn created_then_remote_edited_scheme_survives_with_edits() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Shared", &["one"]);
    h.settle();
    h.assert_scheme_active(D1, scheme);

    // D1 edits the scheme it discovered from D0.
    h.edit_line(D1, scheme, 0, "one edited");
    h.append_line(D1, scheme, "two");
    h.settle();

    h.assert_all_converged();
    for device in h.device_keys() {
        assert!(
            h.device(device).workspace.schemes.contains_key(&scheme),
            "{device:?} lost a scheme created elsewhere then locally edited",
        );
    }
    h.assert_scheme_items_unordered(D0, scheme, &["one edited", "two"]);
    h.assert_scheme_items_unordered(D1, scheme, &["one edited", "two"]);
}

/// Bug #3 (data path): an imported Google calendar's account association
/// (provider/account/email/calendar) must survive sync to a peer — the prerequisite
/// for displaying it there. Exercised with surrounding workspace churn so a lost or
/// diverged node payload is caught.
#[test]
fn imported_calendar_source_syncs_to_peer() {
    let mut h = Harness::new(2);
    h.login_all();

    let cal = h.import_calendar_scheme(
        D0,
        "Work Calendar",
        "google-account-1",
        "user@example.com",
        "calendar-123",
        &["Standup 9am", "1:1 2pm"],
    );
    // A concurrent, unrelated workspace edit on the peer adds churn around the
    // imported calendar's node entry.
    let _other = h.add_scheme(D1, "Notes", &["misc"]);
    h.settle();

    h.assert_all_converged();
    let source = h
        .imported_calendar_source(D1, cal)
        .expect("peer must materialize the imported-calendar source");
    assert_eq!(source.account_id, "google-account-1");
    assert_eq!(source.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(source.calendar_id, "calendar-123");
    assert!(source.read_only, "imported calendar must stay read-only");
}

/// Bug #4: archiving a folder must keep the folder structure — the folder should
/// appear in the archive *as a folder* containing its schemes, not be flattened into
/// a loose list of schemes.
#[test]
fn archived_folder_keeps_structure() {
    let mut h = Harness::new(2);
    h.login_all();

    let folder = h.add_folder(D0, "Projects");
    let alpha = h.add_scheme_to_folder(D0, folder, "Alpha", &["a1"]);
    let beta = h.add_scheme_to_folder(D0, folder, "Beta", &["b1"]);
    h.settle();
    h.assert_all_converged();

    h.archive_folder(D0, folder);
    h.settle();

    h.assert_all_converged();
    // The archived folder survives as a folder containing Alpha and Beta on the peer.
    h.assert_archived_folder_with_schemes(D1, folder, &[alpha, beta]);
}
