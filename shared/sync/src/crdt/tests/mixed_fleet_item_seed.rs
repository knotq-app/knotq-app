//! What a MIXED FLEET does when two builds derive the item-skeleton clientID
//! differently — the question any change to `stable_item_seed_client_id` has to
//! answer before it ships.
//!
//! The deterministic skeleton exists so that two devices which independently
//! create the SAME item emit byte-identical ops, and Yjs dedupes them into one
//! container instead of keeping two. Derived (v8) item ids make that routine
//! rather than rare: starter content and daily carryover mint the same id on every
//! install, so two devices creating one row is the normal case, not a race.
//!
//! Measured at the Yjs level on purpose. `scheme_items()` needs an order entry and
//! a snapshot to materialize a row, and wrapping this in that machinery would
//! measure the materializer; what is in question is whether the two builds' writes
//! merge into one container at all.

use super::super::encoding::{stable_item_seed_client_id, ITEM_SEED_NAMESPACE_BIT};
use super::super::scheme_content::build_item_creation_update_under_seed;
use super::super::*;
use sha2::{Digest, Sha256};
use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, Map, Transact, Update};

/// Exactly what the held-back change computes: namespace v2, document mixed in.
fn new_build_seed(document: DocumentId, item_id: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"knotq.crdt.item_seed_client_id.v2");
    hasher.update(document.0.as_bytes());
    hasher.update(item_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    (u64::from_le_bytes(bytes) & (ITEM_SEED_NAMESPACE_BIT - 1)) | ITEM_SEED_NAMESPACE_BIT | 1
}

/// Merge both devices' creation updates into one document and report what the row
/// ended up holding.
fn merged_text(document: DocumentId, item_id: &str, a: &[u8], b: &[u8]) -> (usize, String) {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(Update::decode_v1(a).unwrap()).unwrap();
        txn.apply_update(Update::decode_v1(b).unwrap()).unwrap();
    }
    let items = doc.get_or_insert_map("items_by_id");
    let txn = doc.transact();
    let keys = items.len(&txn) as usize;
    let text = items
        .get(&txn, item_id)
        .and_then(|value| value.cast::<yrs::MapRef>().ok())
        .and_then(|item| item.get(&txn, "text"))
        .and_then(|value| value.cast::<yrs::TextRef>().ok())
        .map(|text| text.get_string(&txn))
        .unwrap_or_default();
    let _ = document;
    (keys, text)
}

/// The control. If this fails, the test below proves nothing.
#[test]
fn two_devices_on_one_build_creating_one_row_agree() {
    let document = DocumentId::new();
    let item_id = "00000000-0000-8000-8000-000000004010";
    let content = vec![Inline::text("shopping list")];
    let seed = stable_item_seed_client_id(item_id);

    let a = build_item_creation_update_under_seed(document, item_id, &content, seed).unwrap();
    let b = build_item_creation_update_under_seed(document, item_id, &content, seed).unwrap();
    assert_eq!(
        a, b,
        "one build must encode this creation identically every time"
    );

    let (rows, text) = merged_text(document, item_id, &a, &b);
    eprintln!("[same build]  rows={rows} text={text:?}");
    assert_eq!(rows, 1);
    assert_eq!(text, "shopping list", "same-build dedupe is broken");
}

/// **The measured answer, and the evidence behind `app/TODO.md` 0C.** Changing the
/// derivation does not merely risk a duplicate: across a sample of documents it
/// leaves the row's text EMPTY in a large fraction of them, and which way any one
/// document goes is not predictable from anything a user can see.
///
/// The mechanism. Each build's update carries its own skeleton *and* its text, so
/// naively either container would arrive with its own content. But the text is
/// authored under `stable_item_creation_client_id`, which both builds derive
/// identically — so the two text runs occupy the SAME `(clientID, clock)` range
/// while hanging off DIFFERENT parents. Yjs keeps whichever integrated first, and
/// the two containers then compete for the one `items_by_id` key, resolved by last
/// writer. When the surviving container is not the one the surviving text attached
/// to, the row goes blank. That is a starter line or a carried-over Daily line
/// emptying itself for everyone on one of the two versions.
///
/// Asserted as the hazard rather than the property, deliberately, so it passes while
/// the derivation is unchanged and FAILS the moment someone changes it — at which
/// point the change needs a fleet-wide upgrade or a capability gate, as the epoch
/// squash did, and this assertion should be inverted as part of that work.
///
/// Isolates the SKELETON half. The held-back change also bumps
/// `ITEM_CREATION_ENCODING_VERSION`; that cannot rescue this, because which
/// container survives is already decided by the skeleton.
#[test]
fn changing_the_item_seed_derivation_loses_the_row_for_a_mixed_fleet() {
    let item_id = "00000000-0000-8000-8000-000000004010";
    let content = vec![Inline::text("shopping list")];

    // Deterministic document ids, so the sample is the same on every run.
    let documents: Vec<DocumentId> = (0u8..40)
        .map(|n| {
            let mut bytes = [0u8; 16];
            bytes[0] = 0x4b;
            bytes[15] = n;
            DocumentId(uuid::Uuid::from_bytes(bytes))
        })
        .collect();

    let old_seed = stable_item_seed_client_id(item_id);
    let mut blanked = 0usize;
    for document in &documents {
        let new_seed = new_build_seed(*document, item_id);
        assert_ne!(old_seed, new_seed, "the change does alter the derivation");
        let from_old =
            build_item_creation_update_under_seed(*document, item_id, &content, old_seed).unwrap();
        let from_new =
            build_item_creation_update_under_seed(*document, item_id, &content, new_seed).unwrap();
        let (rows, text) = merged_text(*document, item_id, &from_old, &from_new);
        assert_eq!(
            rows, 1,
            "the row itself survives; it is the content that does not"
        );
        if text.is_empty() {
            blanked += 1;
        }
    }
    eprintln!(
        "[mixed fleet] {blanked} of {} documents lost the row's text entirely",
        documents.len()
    );
    assert!(
        blanked > 0,
        "the mixed-fleet hazard this pins has changed shape: no document lost its \
         text. Re-measure before trusting TODO.md 0C's reasoning."
    );
}
