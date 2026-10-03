//! One property: **a workspace-index write never deletes something the writer
//! merely does not have.**
//!
//! The index is the account's — a removal from it is published to every device,
//! and the node it removes leaves its document on the server with nothing
//! addressing it. `sync_string_map` used to mean "the account's index is now
//! exactly this", which is the wrong sentence for a device holding a partial
//! view: a Daily page outside its loaded window, or a scheme its last pull
//! dropped, is absent from its snapshot and present in the account.
//!
//! The same inference is already refused for item content — "the plain copy
//! lacks it" is not a deletion, because the plain files are a projection that
//! lags every pull. These tests hold the index to that rule too. A real delete
//! is explicit and carries a permanent-delete tombstone, which is what
//! distinguishes the two.

mod common;

use common::{Harness, D0, D1};

/// A device whose plain workspace lost a scheme (no tombstone, document intact)
/// must not take it away from the account.
///
/// **Open, and the first attempt at it is recorded here so it is not repeated
/// as-is.** Making `sync_string_map` retain keys the writer has no
/// permanent-delete tombstone for fixes chaos 194 and wedges **30 of 30**
/// otherwise-green single-account seeds: a retained entry materializes back
/// into the workspace, the plain and CRDT halves can then never re-converge, so
/// `workspace_index_mismatch` is true on every pull and the repair queues
/// another full index snapshot each time. Every device ends with an undrained
/// index queue ("still has 12 unpushed edit(s) after settling (wedged)").
///
/// The lesson is that retention has to come with a matching change to the
/// comparison: a key the device retains but cannot speak for must not read as
/// "the halves disagree". The permanent-delete sentinel alone is also too
/// strict a test for removability — archive flows remove a node with an
/// ordinary origin — so the evidence rule needs widening at the same time.
#[test]
#[ignore = "retention without a matching comparison wedges 30/30 seeds; see the doc comment"]
fn an_index_write_does_not_remove_a_scheme_the_writer_merely_lacks() {
    let mut h = Harness::new(2);
    h.login_all();
    h.add_scheme(D0, "kept", &["one"]);
    let doomed = h.add_scheme(D0, "dropped by the pull", &["two"]);
    h.settle();
    assert!(h.device(D1).has_scheme_named("dropped by the pull"));

    // Device 1 holds a partial view, exactly as a pull-dropped scheme leaves it.
    h.drop_scheme_from_plain_only(D1, doomed);
    h.record_workspace_change_pub(D1);
    h.settle();

    for device in h.device_keys() {
        assert!(
            h.device(device).has_scheme_named("kept"),
            "{device:?} lost the scheme nobody touched"
        );
        assert!(
            h.device(device).has_scheme_named("dropped by the pull"),
            "{device:?}: a device that merely lacked the scheme deleted it for the account"
        );
    }
}

/// The guard on the test above: an explicit delete must still reach everyone.
/// The easy way to "fix" absence-as-deletion is to stop removing anything, and
/// then a scheme the user deleted comes back.
#[test]
fn an_explicit_delete_still_removes_the_scheme_everywhere() {
    let mut h = Harness::new(2);
    h.login_all();
    h.add_scheme(D0, "kept", &["one"]);
    let doomed = h.add_scheme(D0, "really deleted", &["two"]);
    h.settle();
    assert!(h.device(D1).has_scheme_named("really deleted"));

    h.delete_scheme(D0, doomed);
    h.settle();

    for device in h.device_keys() {
        assert!(
            h.device(device).has_scheme_named("kept"),
            "{device:?} lost the wrong scheme"
        );
        assert!(
            !h.device(device).has_scheme_named("really deleted"),
            "{device:?} kept a scheme the user deleted"
        );
    }
}
