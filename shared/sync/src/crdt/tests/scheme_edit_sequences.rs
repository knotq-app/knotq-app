//! Randomized edit sequences against one scheme document.
//!
//! `replace_scheme` carries two caches that the per-keystroke work depends on,
//! and both of them decide things a delete needs:
//!
//!  - the **shadow** — the read-back of what this device last wrote — is reused
//!    when the scheme still holds the same items in the same order, and reusing
//!    it *skips the stale-key sweep*, which is what tombstones an item the user
//!    removed. A sequence that got the shadow's validity wrong would silently
//!    leave a deleted item alive in the document.
//!  - the outgoing delta is **captured from the writes** rather than diffed
//!    afterwards, so it carries only that transaction's deletes instead of the
//!    document's whole delete set. That is equivalent only if every update the
//!    device produced reaches the receiver, in order.
//!
//! So each step here checks both: the document must materialize to exactly the
//! scheme it was given, and a peer fed nothing but the emitted updates must
//! arrive at the same items.

use super::super::*;

use knotq_model::{Item, ItemMarker};

/// Deterministic, seeded, tiny — the sequences matter, not the generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn item_texts(items: &[Item]) -> Vec<String> {
    items.iter().map(|item| item.text()).collect()
}

/// One random edit to `scheme`, biased towards deletion — the operation the
/// caches make decisions about, and the one that was reported reappearing.
fn mutate(rng: &mut Rng, scheme: &mut Scheme, retired: &mut Vec<Item>, step: usize) {
    let len = scheme.items.len();
    match rng.below(10) {
        // Delete a line.
        0..=3 if len > 1 => {
            let at = rng.below(len);
            retired.push(scheme.items.remove(at));
        }
        // Delete the *last* line specifically: holding option-backspace walks
        // backwards through the block, so the tail is where deletes cluster.
        4 if len > 1 => {
            retired.push(scheme.items.pop().expect("non-empty"));
        }
        // Put a previously deleted item back, keeping its id — an undo. The
        // document holds it tombstoned, so this has to un-tombstone it.
        5 if !retired.is_empty() => {
            let item = retired.remove(rng.below(retired.len()));
            let at = rng.below(len + 1);
            scheme.items.insert(at, item);
        }
        // Insert a new line.
        6 | 7 => {
            let at = rng.below(len + 1);
            scheme
                .items
                .insert(at, Item::new(format!("inserted at step {step}")));
        }
        // Rewrite a line's text.
        8 if len > 0 => {
            let at = rng.below(len);
            scheme.items[at].set_text(format!("rewritten at step {step}"));
        }
        // Change a line's marker — metadata rather than content.
        9 if len > 0 => {
            let at = rng.below(len);
            scheme.items[at].marker = if scheme.items[at].marker == ItemMarker::Blank {
                ItemMarker::Checkbox
            } else {
                ItemMarker::Blank
            };
        }
        // Nothing changed: a pass that must write nothing and emit nothing.
        _ => {}
    }
}

fn seeded_scheme() -> Scheme {
    let mut scheme = Scheme::new("Notes", 0);
    for n in 0..6 {
        scheme.items.push(Item::new(format!("line {n}")));
    }
    scheme
}

/// The document must equal the scheme after every step, and a peer must reach
/// the same place from the emitted updates alone.
#[test]
fn a_document_and_a_peer_track_a_random_edit_sequence() {
    for seed in 1..=48u64 {
        let document = DocumentId::new();
        let doc = YrsSchemeDocument::new(document);
        let peer = YrsSchemeDocument::new(document);
        let mut scheme = seeded_scheme();
        let mut retired: Vec<Item> = Vec::new();
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);

        // Seed both sides from the same starting state.
        let seed_update = doc
            .sync_scheme(&scheme)
            .expect("seed")
            .expect("a seed update");
        peer.apply_update_v1(&seed_update.update_v1)
            .expect("seed peer");

        for step in 0..40 {
            mutate(&mut rng, &mut scheme, &mut retired, step);
            let update = doc.sync_scheme(&scheme).expect("sync_scheme");

            let materialized = doc.scheme_items().expect("materialize");
            assert_eq!(
                item_texts(&materialized),
                item_texts(&scheme.items),
                "seed {seed} step {step}: the document does not match the scheme it was given"
            );

            if let Some(update) = update {
                peer.apply_update_v1(&update.update_v1)
                    .expect("apply to peer");
            }
            let peer_items = peer.scheme_items().expect("materialize peer");
            assert_eq!(
                item_texts(&peer_items),
                item_texts(&scheme.items),
                "seed {seed} step {step}: a peer fed only the emitted updates diverged"
            );
            doc.validate()
                .unwrap_or_else(|err| panic!("seed {seed} step {step}: invalid document: {err:#}"));
        }
    }
}

/// The narrow case the shadow is built for: repeated passes over a scheme whose
/// item set never moves, interleaved with a delete. The pass after a delete is
/// the one that must not reuse a shadow describing the old item set, and the
/// passes after *that* must not skip the sweep on a document that still has
/// something to sweep.
#[test]
fn repeated_no_op_passes_around_a_delete_still_tombstone_it() {
    let document = DocumentId::new();
    let doc = YrsSchemeDocument::new(document);
    let peer = YrsSchemeDocument::new(document);
    let mut scheme = seeded_scheme();

    let seed = doc
        .sync_scheme(&scheme)
        .expect("seed")
        .expect("a seed update");
    peer.apply_update_v1(&seed.update_v1).expect("seed peer");

    for round in 0..5 {
        // Passes that change nothing: these are what leave a reusable shadow.
        for _ in 0..3 {
            assert!(
                doc.sync_scheme(&scheme).expect("no-op pass").is_none(),
                "round {round}: a pass that changed nothing emitted an update"
            );
        }
        let removed = scheme.items.pop().expect("items left");
        let update = doc
            .sync_scheme(&scheme)
            .expect("delete pass")
            .expect("a delete must emit an update");
        peer.apply_update_v1(&update.update_v1).expect("apply");

        for (label, items) in [
            ("document", doc.scheme_items().expect("materialize")),
            ("peer", peer.scheme_items().expect("materialize peer")),
        ] {
            assert!(
                !items.iter().any(|item| item.id == removed.id),
                "round {round}: the {label} still holds the deleted line {:?}",
                removed.text()
            );
            assert_eq!(item_texts(&items), item_texts(&scheme.items));
        }
    }
}

/// Deleting several lines in a row with no pass in between them and the peer —
/// the "held option-backspace through a block" shape — must reach the peer as
/// one coherent sequence.
#[test]
fn a_run_of_deletes_reaches_a_peer_line_for_line() {
    let document = DocumentId::new();
    let doc = YrsSchemeDocument::new(document);
    let peer = YrsSchemeDocument::new(document);
    let mut scheme = seeded_scheme();

    let seed = doc
        .sync_scheme(&scheme)
        .expect("seed")
        .expect("a seed update");
    peer.apply_update_v1(&seed.update_v1).expect("seed peer");

    let mut queued = Vec::new();
    while scheme.items.len() > 1 {
        scheme.items.pop();
        if let Some(update) = doc.sync_scheme(&scheme).expect("delete pass") {
            queued.push(update.update_v1);
        }
    }
    // The peer only hears about them once, at the end — as it would after being
    // offline for the whole burst.
    for update in &queued {
        peer.apply_update_v1(update).expect("apply");
    }

    assert_eq!(
        item_texts(&doc.scheme_items().unwrap()),
        item_texts(&scheme.items)
    );
    assert_eq!(
        item_texts(&peer.scheme_items().unwrap()),
        item_texts(&scheme.items),
        "a peer that received the whole burst at once ended up with different lines"
    );
}
