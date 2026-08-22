//! A deleted line must be tombstoned ONCE, not re-tombstoned on every later edit.
//!
//! Yjs records a fresh last-writer-wins entry for every map write, so
//! re-asserting `deleted = true` is not free: each rewrite is bytes pushed to
//! the server and kept in the document's history forever. Re-tombstoning on
//! every pass meant a scheme the user had deleted lines from paid that cost on
//! every keystroke, permanently — and the cost scaled with how many lines had
//! been deleted.

mod common;

use common::{Harness, D0, D1};

/// Growth from editing one surviving line, in a scheme with `deleted` lines
/// removed beforehand.
fn growth_with_deleted_lines(deleted: usize, edits: usize) -> usize {
    let mut h = Harness::new(1);
    h.login_all();

    let mut lines: Vec<String> = (0..deleted).map(|i| format!("remove me {i}")).collect();
    lines.push("survivor".to_string());
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let scheme = h.add_scheme(D0, "Notes", &refs);
    h.sync(D0);

    // Delete every line except the survivor, establishing the tombstones.
    for _ in 0..deleted {
        h.remove_line(D0, scheme, 0);
    }
    h.sync(D0);
    let baseline = h.device(D0).scheme_state_len(scheme);

    // Edit ONLY the survivor. Nothing here concerns the deleted lines.
    for i in 0..edits {
        h.edit_line(D0, scheme, 0, &format!("survivor {i}"));
        h.sync(D0);
    }
    h.device(D0).scheme_state_len(scheme) - baseline
}

/// The cost of editing a line must not depend on how many OTHER lines were
/// deleted earlier. Comparing two schemes rather than asserting an absolute
/// byte count keeps this robust to encoding jitter: the real edits cost the
/// same in both, so any difference is tombstone rewriting.
#[test]
fn editing_cost_does_not_scale_with_deleted_lines() {
    const EDITS: usize = 8;
    let few = growth_with_deleted_lines(1, EDITS);
    let many = growth_with_deleted_lines(12, EDITS);

    assert!(
        many < few + (EDITS * 12),
        "editing a line cost {many} bytes with 12 deleted lines vs {few} with 1 — \
         tombstones are being rewritten on every pass (about 11 bytes each, \
         pushed to the server and kept forever)"
    );
}

/// The obvious wrong version of the fix — never tombstoning — must not pass.
#[test]
fn deleting_a_line_still_takes_effect_and_converges() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Notes", &["alpha", "beta", "gamma"]);
    h.sync(D0);
    h.sync(D1);

    h.remove_line(D0, scheme, 1);
    h.sync(D0);
    h.sync(D1);

    for device in [D0, D1] {
        assert_eq!(
            h.device(device).scheme_line_count("Notes"),
            Some(2),
            "device {device:?} should see the delete"
        );
    }
    assert!(
        h.device(D0).converges_with(h.device(D1)),
        "devices diverged after a delete"
    );
}
