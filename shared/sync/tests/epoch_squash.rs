//! Epoch-based history squash: adoption-by-replacement through the real engine.
//!
//! A squash replaces a scheme document's server state with a freshly rebuilt
//! CRDT of identical logical content but no edit history, bumping the document
//! epoch. These tests drive the ADOPTION side — the part every client runs —
//! through the real `batch_pull_and_apply`/`batch_push_pending` engine against
//! the in-memory `TestServer` (which mirrors the backend's epoch bookkeeping:
//! per-document epoch, stale-epoch push rejection, squash = replace + bump).

mod common;

use common::{Harness, D0, D1, D2};
use knotq_sync::SyncPushEpochStale;

#[test]
fn squashed_document_is_adopted_by_replacement_without_doubling() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Notes", &["alpha", "beta", "gamma"]);
    h.settle();
    h.assert_scheme_items(D1, scheme, &["alpha", "beta", "gamma"]);

    // Some real history so the rebuild differs from the stored state.
    h.edit_line(D0, scheme, 1, "beta v2");
    h.edit_line(D0, scheme, 1, "beta v3");
    h.settle();

    // D0 (fully synced) squashes on the server.
    let (_, epoch) = h.squash_scheme_on_server(D0, scheme);
    assert_eq!(epoch, 1);

    // Both devices pull the squashed state. A naive CRDT merge would double
    // every item's text (the squashed doc shares no Yjs history); adoption
    // replaces instead.
    h.sync(D0);
    h.sync(D1);
    h.assert_all_converged();
    h.assert_scheme_items(D0, scheme, &["alpha", "beta v3", "gamma"]);
    h.assert_scheme_items(D1, scheme, &["alpha", "beta v3", "gamma"]);
    assert_eq!(h.scheme_epoch(D0, scheme), 1);
    assert_eq!(h.scheme_epoch(D1, scheme), 1);

    // Post-squash editing works: new deltas carry the new epoch and merge.
    h.edit_line(D1, scheme, 0, "alpha edited");
    h.settle();
    h.assert_all_converged();
    h.assert_scheme_items(D0, scheme, &["alpha edited", "beta v3", "gamma"]);
}

#[test]
fn pending_local_edits_survive_adoption() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Plan", &["one", "two", "three"]);
    h.settle();

    let (_, epoch) = h.squash_scheme_on_server(D0, scheme);
    assert_eq!(epoch, 1);

    // Before D1 ever sees the squash, it edits a line AND adds a line — both
    // become old-epoch pending deltas that cannot merge into the adopted doc.
    h.edit_line(D1, scheme, 1, "two edited");
    h.append_line(D1, scheme, "four");

    // One sync: pull adopts the squashed state, the rescue re-expresses both
    // local edits against it, and the push lands them at the new epoch.
    h.sync(D1);
    h.sync(D0);
    h.assert_all_converged();
    h.assert_scheme_items(D0, scheme, &["one", "two edited", "three", "four"]);
}

#[test]
fn concurrent_remote_edit_survives_anothers_adoption() {
    let mut h = Harness::new(3);
    h.login_all();

    let scheme = h.add_scheme(D0, "Shared", &["a", "b", "c"]);
    h.settle();

    h.squash_scheme_on_server(D0, scheme);

    // D2 adopts immediately and edits line 2 at the new epoch.
    h.sync(D2);
    h.edit_line(D2, scheme, 2, "c remote");
    h.sync(D2);

    // D1 is still at epoch 0 and has an old-epoch pending edit on line 0. Its
    // pull merges the squash AND D2's post-squash edit; the rescue must keep
    // D2's edit (item untouched locally) alongside its own.
    h.edit_line(D1, scheme, 0, "a local");
    h.sync(D1);
    h.settle();
    h.assert_all_converged();
    h.assert_scheme_items(D0, scheme, &["a local", "b", "c remote"]);
}

#[test]
fn local_deletion_and_remote_deletion_both_survive_adoption() {
    let mut h = Harness::new(3);
    h.login_all();

    let scheme = h.add_scheme(D0, "List", &["keep", "remote-del", "local-del", "tail"]);
    h.settle();

    // D2 deletes "remote-del" and pushes; THEN the squash captures that state.
    h.remove_line(D2, scheme, 1);
    h.settle();
    h.squash_scheme_on_server(D0, scheme);

    // D1, still at epoch 0, deletes "local-del" (old-epoch pending) and syncs.
    // Adoption must honor the local deletion (touched + locally absent) and the
    // remote one (untouched + remotely absent). ("local-del" is at index 1 now:
    // the settle above already applied D2's deletion everywhere.)
    h.remove_line(D1, scheme, 1);
    h.sync(D1);
    h.settle();
    h.assert_all_converged();
    h.assert_scheme_items(D1, scheme, &["keep", "tail"]);
}

#[test]
fn stale_epoch_push_is_rejected_typed_then_recovers_on_next_sync() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Race", &["x", "y"]);
    h.settle();

    // D1 queues an edit, then the squash lands BEFORE D1 pushes (the race the
    // full sync loop cannot hit because its pull runs first).
    h.edit_line(D1, scheme, 0, "x edited");
    h.squash_scheme_on_server(D0, scheme);

    let err = h
        .device_push_only(D1)
        .expect_err("old-epoch push must be rejected");
    assert!(
        err.downcast_ref::<SyncPushEpochStale>().is_some(),
        "expected typed SyncPushEpochStale, got: {err:#}"
    );
    // The pending edit must NOT have been dropped by the failed push.
    assert!(h.device(D1).pending_count() > 0);

    // The driver's response to the typed error is one more full cycle: the pull
    // adopts and rescues, then the push succeeds.
    h.sync(D1);
    h.sync(D0);
    h.assert_all_converged();
    h.assert_scheme_items(D0, scheme, &["x edited", "y"]);
    assert_eq!(h.scheme_epoch(D1, scheme), 1);
}

#[test]
fn squash_of_untouched_document_leaves_other_documents_alone() {
    let mut h = Harness::new(2);
    h.login_all();

    let squashed = h.add_scheme(D0, "Big", &["big content"]);
    let bystander = h.add_scheme(D0, "Small", &["small content"]);
    h.settle();

    h.squash_scheme_on_server(D0, squashed);
    h.edit_line(D1, bystander, 0, "small content edited");
    h.settle();
    h.assert_all_converged();
    h.assert_scheme_items(D0, bystander, &["small content edited"]);
    h.assert_scheme_items(D0, squashed, &["big content"]);
    assert_eq!(h.scheme_epoch(D0, bystander), 0);
    assert_eq!(h.scheme_epoch(D0, squashed), 1);
}
