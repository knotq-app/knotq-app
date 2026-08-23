//! A marker family chosen on one device must reach the others.
//!
//! The family rides in the item snapshot rather than in its own CRDT field, so
//! this checks the claim rather than assuming it — a field that serialized but
//! never synced would look correct on the device that set it and nowhere else.

mod common;

use common::{Harness, D0, D1};
use knotq_model::{ItemMarker, MarkerFamily};

#[test]
fn a_marker_family_converges_to_other_devices() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Lists", &["first", "second"]);
    h.sync(D0);
    h.sync(D1);

    h.device_mut_for_surgery(D0).set_marker_family(
        scheme,
        0,
        ItemMarker::Numbered,
        MarkerFamily::Roman,
    );
    h.device_mut_for_surgery(D0).set_marker_family(
        scheme,
        1,
        ItemMarker::Bullet,
        MarkerFamily::Squares,
    );
    h.sync(D0);
    h.sync(D1);

    let seen = h.device(D1).workspace.schemes[&scheme].items.clone();
    assert_eq!(seen[0].marker, ItemMarker::Numbered);
    assert_eq!(
        seen[0].marker_family,
        MarkerFamily::Roman,
        "the numbered family did not reach the other device"
    );
    assert_eq!(seen[1].marker, ItemMarker::Bullet);
    assert_eq!(
        seen[1].marker_family,
        MarkerFamily::Squares,
        "the bullet family did not reach the other device"
    );
    assert!(h.device(D0).converges_with(h.device(D1)));
}
