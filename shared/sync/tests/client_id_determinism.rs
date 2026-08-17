//! Yjs clientIDs are a merge input, so a fuzz run whose clientIDs came from the
//! OS CSPRNG cannot be replayed from its seed — which is what made a rare
//! convergence failure impossible to debug. `random_document_client_id` now
//! mints through the model, so `set_deterministic_id_seed` covers it.
//!
//! Both halves matter and are checked here: seeded runs must be *reproducible*,
//! and unseeded (production) runs must still be *fresh-random per construction*
//! — reusing a clientID across incarnations reintroduces the `(clientID, clock)`
//! aliasing that broke multi-origin daily-queue merges.

use std::collections::HashSet;

use knotq_model::{
    set_deterministic_id_seed, Item, NodeRef, Scheme, Workspace,
};
use knotq_sync::WorkspaceCrdtDocuments;

fn workspace_with_schemes(count: usize) -> Workspace {
    let mut workspace = Workspace::new();
    for i in 0..count {
        let mut scheme = Scheme::new(format!("scheme-{i}"), 0);
        scheme.items = vec![Item::new("alpha"), Item::new("beta")];
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme.id));
        workspace.schemes.insert(scheme.id, scheme);
    }
    workspace.ensure_sync_metadata();
    workspace
}

/// The encoded document bytes carry the clientID, so identical bytes from two
/// runs of the same seed is exactly the reproducibility the fuzzer needs.
fn encoded_under_seed(seed: u64) -> Vec<Vec<u8>> {
    set_deterministic_id_seed(Some(seed));
    let workspace = workspace_with_schemes(4);
    let docs = WorkspaceCrdtDocuments::try_new(&workspace).expect("build documents");
    let states = docs.document_states();
    let mut out: Vec<Vec<u8>> = states.into_values().collect();
    out.sort();
    set_deterministic_id_seed(None);
    out
}

#[test]
fn a_seeded_run_reproduces_identical_documents() {
    let first = encoded_under_seed(4242);
    let second = encoded_under_seed(4242);
    assert_eq!(
        first, second,
        "the same seed must produce byte-identical CRDT documents, or a failing \
         fuzz seed cannot be replayed"
    );
    assert!(!first.is_empty());
}

#[test]
fn different_seeds_produce_different_documents() {
    assert_ne!(
        encoded_under_seed(1),
        encoded_under_seed(2),
        "distinct seeds must explore distinct scenarios"
    );
}

/// Without a seed — i.e. in production — every construction must still get its
/// own fresh clientID.
#[test]
fn unseeded_documents_get_fresh_client_ids_each_construction() {
    set_deterministic_id_seed(None);
    let workspace = workspace_with_schemes(2);
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    for _ in 0..8 {
        let docs = WorkspaceCrdtDocuments::try_new(&workspace).expect("build documents");
        let mut states: Vec<Vec<u8>> = docs.document_states().into_values().collect();
        states.sort();
        assert!(
            seen.insert(states.concat()),
            "two constructions produced identical clientIDs; fresh-random per \
             construction is load-bearing for multi-origin merge correctness"
        );
    }
}

/// Seeding must be scoped to the thread that asked for it, so a background sync
/// thread never inherits a test's deterministic stream.
#[test]
fn the_deterministic_seed_does_not_leak_across_threads() {
    set_deterministic_id_seed(Some(99));
    let seeded = encoded_under_seed(99);
    let unseeded = std::thread::spawn(|| {
        let workspace = workspace_with_schemes(4);
        let docs = WorkspaceCrdtDocuments::try_new(&workspace).expect("build documents");
        let mut states: Vec<Vec<u8>> = docs.document_states().into_values().collect();
        states.sort();
        states
    })
    .join()
    .unwrap();
    set_deterministic_id_seed(None);
    assert_ne!(
        seeded, unseeded,
        "another thread must keep random ids while this one is seeded"
    );
}

/// Every document this device authors must take its clientID from the *document*
/// half of the partitioned clientID space.
///
/// The two halves (see `ITEM_SEED_NAMESPACE_BIT`) exist so a document's
/// text-content struct can never alias an item-skeleton struct: aliasing a
/// `(clientID, clock)` makes the Yjs merge order-dependent, which is how the
/// multi-origin daily-queue divergence happened. The workspace document was
/// built with a bare `Doc::new()` and so took yrs's *unpartitioned* default —
/// outside both halves — while the scheme document had always been correct.
#[test]
fn every_authored_document_takes_a_partitioned_client_id() {
    // Mirrors `ITEM_SEED_NAMESPACE_BIT`; a document clientID must have it clear.
    const ITEM_SEED_NAMESPACE_BIT: u64 = 1 << 52;

    set_deterministic_id_seed(None);
    let workspace = workspace_with_schemes(3);
    for _ in 0..8 {
        let docs = WorkspaceCrdtDocuments::try_new(&workspace).expect("build documents");
        for client_id in docs.probe_client_ids() {
            assert!(
                client_id & ITEM_SEED_NAMESPACE_BIT == 0,
                "clientID {client_id:#x} is outside the document namespace — it can \
                 alias an item-skeleton struct and make merges order-dependent"
            );
            assert!(client_id != 0, "clientID must be non-zero");
        }
    }
}
