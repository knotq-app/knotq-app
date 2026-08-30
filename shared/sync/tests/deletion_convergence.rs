//! Deleting lines in quick succession must not come back.
//!
//! The shapes here are the ones a user produces holding option-backspace
//! through a block of text: several trailing deletes with no sync in between,
//! one delete per sync round trip, and the word-by-word shrink that ends in a
//! line merge. Each is checked through the real engine, and again after the
//! self-echo pull that a push always leaves behind — the round trip that used to
//! be blamed for lines reappearing.

mod common;

use common::{Harness, D0};

fn texts(h: &Harness, scheme: knotq_model::SchemeId) -> Vec<String> {
    h.device(D0).scheme_item_texts(scheme)
}

/// Delete the last line repeatedly with no sync in between, then sync once.
#[test]
fn rapid_trailing_deletes_then_one_sync() {
    let mut h = Harness::new(1);
    h.login_all();
    let scheme = h.add_scheme(D0, "Notes", &["one", "two", "three", "four", "five"]);
    h.sync(D0);

    for _ in 0..3 {
        let last = texts(&h, scheme).len() - 1;
        h.remove_line(D0, scheme, last);
    }
    assert_eq!(texts(&h, scheme), vec!["one", "two"]);

    h.sync(D0);
    assert_eq!(texts(&h, scheme), vec!["one", "two"], "after first sync");
    h.sync(D0);
    assert_eq!(texts(&h, scheme), vec!["one", "two"], "after echo sync");
    h.sync(D0);
    assert_eq!(texts(&h, scheme), vec!["one", "two"], "after third sync");
}

/// Delete a line, sync, delete another, sync — the "sync keeps up with the
/// typing" shape.
#[test]
fn deletes_interleaved_with_syncs() {
    let mut h = Harness::new(1);
    h.login_all();
    let scheme = h.add_scheme(D0, "Notes", &["one", "two", "three", "four", "five"]);
    h.sync(D0);

    for expected in [
        vec!["one", "two", "three", "four"],
        vec!["one", "two", "three"],
        vec!["one", "two"],
        vec!["one"],
    ] {
        let last = texts(&h, scheme).len() - 1;
        h.remove_line(D0, scheme, last);
        h.sync(D0);
        assert_eq!(texts(&h, scheme), expected);
        h.sync(D0);
        assert_eq!(texts(&h, scheme), expected, "echo sync resurrected a line");
    }
}

/// Word-by-word backspace: the line's text shrinks, then the (now empty) line
/// merges into the one above. That is an edit + a delete in the same scheme,
/// repeated fast.
#[test]
fn option_backspace_word_by_word_then_line_merge() {
    let mut h = Harness::new(1);
    h.login_all();
    let scheme = h.add_scheme(
        D0,
        "Notes",
        &["alpha beta", "gamma delta", "epsilon zeta", "eta theta"],
    );
    h.sync(D0);

    // "eta theta" -> "eta " -> "" -> merged away
    h.edit_line(D0, scheme, 3, "eta ");
    h.edit_line(D0, scheme, 3, "");
    h.remove_line(D0, scheme, 3);
    h.edit_line(D0, scheme, 2, "epsilon ");
    h.edit_line(D0, scheme, 2, "");
    h.remove_line(D0, scheme, 2);

    let expected = vec!["alpha beta", "gamma delta"];
    assert_eq!(texts(&h, scheme), expected, "locally");
    h.sync(D0);
    assert_eq!(texts(&h, scheme), expected, "after sync");
    h.sync(D0);
    assert_eq!(texts(&h, scheme), expected, "after echo");
}
