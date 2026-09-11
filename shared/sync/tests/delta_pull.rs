//! Exercises the optimized state-vector pull response through the real Rust
//! engine and the in-memory server model.

mod common;

use common::{Harness, D0, D1};

#[test]
fn an_existing_device_converges_from_a_state_vector_delta() {
    let mut harness = Harness::new(2);
    harness.login_all();

    let scheme = harness.add_scheme(D0, "Project", &["base"]);
    harness.sync(D0);
    harness.sync(D1);

    // D1 owns the old merged state. D0 makes a later edit, so D1's next pull
    // should receive only the missing Yjs structs from the delta-capable model.
    harness.append_line(D0, scheme, "remote addition");
    harness.sync(D0);
    harness.sync(D1);

    harness.assert_all_converged();
    harness.assert_scheme_items(D1, scheme, &["base", "remote addition"]);
    assert!(
        harness.server_delta_pull_documents() > 0,
        "the test server must have exercised the state-vector delta response"
    );
}
