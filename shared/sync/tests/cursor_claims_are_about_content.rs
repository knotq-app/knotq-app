//! One property: **a pull cursor is a claim about content, and a false claim
//! must not be believed.**
//!
//! Every per-document pull cursor says "I have this document's history through
//! sequence N". The next pull asks only for what comes after N, so if the cursor
//! survives while the document's bytes do not, the content is never re-delivered
//! and the gap is permanent.
//!
//! That state is reachable and was measured in journal-loss seed 20223: a device
//! holding 6 cursors and 6 schemes whose workspace index document was 2 bytes
//! (empty) while its cursor read `pulled=1, pushed=4`. For the workspace index
//! the consequence compounds, because the repair that publishes a device's own
//! index content gives up while the index is unseeded — so the device can never
//! publish, and loses local-only pages the moment the account's index arrives
//! from another device.
//!
//! Dropping a false claim can only cost a re-download, and it cannot loop: every
//! index the server holds carries a `meta.id`, so once the re-pull lands the
//! device is past this state for good.
//!
//! **This test passes with and without a cursor reset in the engine**, because
//! `WorkspaceCrdtDocuments::from_states` repopulates the index from the plain
//! workspace and the harness therefore cannot reach the empty-index state the
//! desktop run did. It is kept as a guard on the property itself — losing the
//! index document's bytes must not cost the account its schemes — and
//! deliberately NOT as evidence for a fix. The engine was left alone because no
//! failing case could be built for it; see `app/TODO.md`.

mod common;

use common::{Harness, D0, D1};

/// The index document's bytes are gone while its cursor still claims a pull.
/// The device must re-pull it rather than treat an empty index as the truth.
#[test]
fn an_index_cursor_without_index_content_is_re_pulled() {
    let mut h = Harness::new(2);
    h.login_all();
    h.add_scheme(D0, "alpha", &["one"]);
    h.add_scheme(D0, "beta", &["two"]);
    h.settle();
    assert!(h.device(D1).has_scheme_named("alpha"));
    assert!(h.device(D1).has_scheme_named("beta"));

    // Device 1 keeps its cursors and loses the index document's bytes — the
    // pairing violation above, with the cursor left untouched.
    let index = h.device(D1).workspace.sync.id;
    assert!(
        h.device(D1)
            .local_state()
            .document_cursors
            .get(&index)
            .is_some_and(|cursor| cursor.last_pulled_sequence > 0),
        "the test needs a cursor that claims a pull"
    );
    h.device_mut_for_surgery(D1)
        .lose_crdt_state_for_document(index);

    h.settle();

    for device in h.device_keys() {
        assert!(
            h.device(device).has_scheme_named("alpha"),
            "{device:?} lost 'alpha' to an index it could not re-pull"
        );
        assert!(
            h.device(device).has_scheme_named("beta"),
            "{device:?} lost 'beta' to an index it could not re-pull"
        );
    }
}
