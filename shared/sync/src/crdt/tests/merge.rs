//! Concurrent-merge and document-roundtrip CRDT unit tests.
use super::super::*;

use knotq_model::{ImageAssetFormat, ImageInline, Item};

#[test]
fn scheme_document_update_can_be_applied_to_empty_replica() {
    let document = DocumentId::new();
    let mut scheme = Scheme::new("Plan", 0);
    scheme.items.push(Item::new("First"));
    scheme.items.push(Item::new("Second"));

    let left = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();
    let right = YrsSchemeDocument::new(document);
    let update = left.encode_update_v1(&right.state_vector_v1()).unwrap();

    right.apply_update_v1(&update).unwrap();

    assert_eq!(right.item_texts().unwrap(), vec!["First", "Second"]);
}

#[test]
fn image_line_roundtrips_through_crdt() {
    let document = DocumentId::new();
    let image = ImageInline {
        asset: uuid::Uuid::new_v4(),
        format: ImageAssetFormat::Png,
        width: Some(640),
        height: Some(360),
    };
    // A line is a single block object: this is an image line, no text.
    let mut item = Item::new("");
    item.set_image(image);
    let expected = item.content.clone();
    let mut scheme = Scheme::new("Plan", 0);
    scheme.items.push(item);

    let doc = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();
    let items = doc.scheme_items().unwrap();

    assert_eq!(items[0].content, expected);
    assert!(items[0].content.is_block());
}

#[test]
fn concurrent_content_edits_to_distinct_items_merge_without_duplicates() {
    let document = DocumentId::new();
    let mut base = Scheme::new("Plan", 0);
    base.items.push(Item::new("First"));
    base.items.push(Item::new("Second"));

    // Two replicas start from the same base state.
    let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
    let base_update = left.encode_update_v1(&[]).unwrap();
    let right = YrsSchemeDocument::new(document);
    right.apply_update_v1(&base_update).unwrap();

    // Each replica edits a *different* item's text concurrently.
    let mut scheme_left = base.clone();
    scheme_left.items[0].set_text("First edited");
    let delta_left = left.sync_scheme(&scheme_left).unwrap().unwrap().update_v1;

    let mut scheme_right = base.clone();
    scheme_right.items[1].set_text("Second edited");
    let delta_right = right.sync_scheme(&scheme_right).unwrap().unwrap().update_v1;

    // A third replica merges both concurrent deltas.
    let merged = YrsSchemeDocument::new(document);
    merged.apply_update_v1(&base_update).unwrap();
    merged.apply_update_v1(&delta_left).unwrap();
    merged.apply_update_v1(&delta_right).unwrap();

    // The order array is not rewritten on a content-only edit, so the merge
    // does not produce duplicate item_order entries and stays schema-valid.
    merged.validate().unwrap();
    assert_eq!(
        merged.item_texts().unwrap(),
        vec!["First edited", "Second edited"]
    );
}

#[test]
fn concurrent_image_embeds_on_distinct_items_merge() {
    let document = DocumentId::new();
    let mut base = Scheme::new("Plan", 0);
    base.items.push(Item::new("First"));
    base.items.push(Item::new("Second"));
    let image_a = ImageInline {
        asset: uuid::Uuid::new_v4(),
        format: ImageAssetFormat::Png,
        width: Some(64),
        height: Some(64),
    };
    let image_b = ImageInline {
        asset: uuid::Uuid::new_v4(),
        format: ImageAssetFormat::Png,
        width: Some(64),
        height: Some(64),
    };

    let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
    let base_update = left.encode_update_v1(&[]).unwrap();
    let right = YrsSchemeDocument::new(document);
    right.apply_update_v1(&base_update).unwrap();

    let mut scheme_left = base.clone();
    scheme_left.items[0].set_image(image_a);
    let delta_left = left.sync_scheme(&scheme_left).unwrap().unwrap().update_v1;

    let mut scheme_right = base.clone();
    scheme_right.items[1].set_image(image_b);
    let delta_right = right.sync_scheme(&scheme_right).unwrap().unwrap().update_v1;

    let merged = YrsSchemeDocument::new(document);
    merged.apply_update_v1(&base_update).unwrap();
    merged.apply_update_v1(&delta_left).unwrap();
    merged.apply_update_v1(&delta_right).unwrap();

    let items = merged.scheme_items().unwrap();
    assert_eq!(
        items[0].images().copied().collect::<Vec<_>>(),
        vec![image_a]
    );
    assert_eq!(
        items[1].images().copied().collect::<Vec<_>>(),
        vec![image_b]
    );
}

#[test]
fn concurrent_edits_to_same_item_text_merge_character_wise() {
    let document = DocumentId::new();
    let mut base = Scheme::new("Plan", 0);
    base.items.push(Item::new("hello"));

    // Two replicas start from the same single-line base.
    let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
    let base_update = left.encode_update_v1(&[]).unwrap();
    let right = YrsSchemeDocument::new(document);
    right.apply_update_v1(&base_update).unwrap();

    // Both edit the *same* line concurrently: left appends, right prepends.
    let mut scheme_left = base.clone();
    scheme_left.items[0].set_text("hello!");
    let delta_left = left.sync_scheme(&scheme_left).unwrap().unwrap().update_v1;

    let mut scheme_right = base.clone();
    scheme_right.items[0].set_text("Xhello");
    let delta_right = right.sync_scheme(&scheme_right).unwrap().unwrap().update_v1;

    // Merge both concurrent edits into a third replica.
    let merged = YrsSchemeDocument::new(document);
    merged.apply_update_v1(&base_update).unwrap();
    merged.apply_update_v1(&delta_left).unwrap();
    merged.apply_update_v1(&delta_right).unwrap();

    merged.validate().unwrap();
    // Because text is a sequence CRDT, both insertions survive instead of one
    // last-writer-wins clobbering the other. Order is deterministic.
    assert_eq!(merged.item_texts().unwrap(), vec!["Xhello!".to_string()]);
}

#[test]
fn identical_concurrent_insert_into_blank_materializes_once() {
    let document = DocumentId::new();
    let mut base = Scheme::new("Plan", 0);
    base.items.push(Item::new(""));

    let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
    let base_update = left.encode_update_v1(&[]).unwrap();
    let right = YrsSchemeDocument::new(document);
    right.apply_update_v1(&base_update).unwrap();

    let mut scheme_left = base.clone();
    scheme_left.items[0].set_text("task A");
    let delta_left = left.sync_scheme(&scheme_left).unwrap().unwrap().update_v1;

    let mut scheme_right = base.clone();
    scheme_right.items[0].set_text("task A");
    let delta_right = right.sync_scheme(&scheme_right).unwrap().unwrap().update_v1;

    let merged = YrsSchemeDocument::new(document);
    merged.apply_update_v1(&base_update).unwrap();
    merged.apply_update_v1(&delta_left).unwrap();
    merged.apply_update_v1(&delta_right).unwrap();

    assert_eq!(merged.item_texts().unwrap(), vec!["task A".to_string()]);
}

#[test]
fn intentional_doubled_text_roundtrips() {
    let document = DocumentId::new();
    let mut scheme = Scheme::new("Plan", 0);
    scheme.items.push(Item::new("task Atask A"));

    let doc = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();

    assert_eq!(doc.item_texts().unwrap(), vec!["task Atask A".to_string()]);
}

#[test]
fn concurrent_inserts_into_same_gap_merge_without_wedge() {
    let document = DocumentId::new();
    let mut base = Scheme::new("Plan", 0);
    base.items.push(Item::new("A"));
    base.items.push(Item::new("B"));

    let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
    let base_update = left.encode_update_v1(&[]).unwrap();
    let right = YrsSchemeDocument::new(document);
    right.apply_update_v1(&base_update).unwrap();

    // Both replicas insert a new item into the *same* gap (between A and B)
    // offline, so they independently generate the same fractional position.
    let mut left_scheme = base.clone();
    left_scheme.items.insert(1, Item::new("X"));
    let delta_left = left.sync_scheme(&left_scheme).unwrap().unwrap().update_v1;

    let mut right_scheme = base.clone();
    right_scheme.items.insert(1, Item::new("Y"));
    let delta_right = right.sync_scheme(&right_scheme).unwrap().unwrap().update_v1;

    let merged = YrsSchemeDocument::new(document);
    merged.apply_update_v1(&base_update).unwrap();
    merged.apply_update_v1(&delta_left).unwrap();
    merged.apply_update_v1(&delta_right).unwrap();

    // Identical positions are fine: the id tiebreak keeps a deterministic
    // total order, both inserts survive, and the schema stays valid.
    merged.validate().unwrap();
    let texts = merged.item_texts().unwrap();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert_eq!(texts[0], "A");
    assert_eq!(texts[3], "B");
    assert!(texts.contains(&"X".to_string()));
    assert!(texts.contains(&"Y".to_string()));
}

/// Feasibility proof for the deterministic-creation fix. Creating an item's
/// *skeleton* (its sub-map + `schema`/`id` + an empty Text) under a fixed,
/// id-derived clientID makes two independent creations byte-identical, so Yjs
/// dedupes them into ONE container instead of clobbering one. Meanwhile each
/// device's *content* edit keeps its own clientID, so concurrent inserts stay
/// distinct and merge (AB/BA) — exactly the property that must NOT regress.
#[test]
fn deterministic_skeleton_dedupes_yet_merges_concurrent_content() {
    use yrs::GetString;
    let document = DocumentId::new();
    let item_id = "11111111-1111-4111-8111-111111111111";
    const SEED_CID: u64 = 0x5EED;

    // The skeleton-create update, generated identically on every device.
    let skeleton = || -> Vec<u8> {
        let doc = Doc::with_options(yrs_doc_options(document, SEED_CID, OffsetKind::Utf16));
        {
            let items = doc.get_or_insert_map("items_by_id");
            let mut txn = doc.transact_mut();
            let item_map = items.insert(&mut txn, item_id, MapPrelim::default());
            item_map.insert(&mut txn, "schema", "knotq.item.v1");
            item_map.insert(&mut txn, "id", item_id);
            item_map.insert(&mut txn, "text", TextPrelim::new(""));
        }
        let update = doc.transact().encode_diff_v1(&StateVector::default());
        update
    };
    assert_eq!(
        skeleton(),
        skeleton(),
        "skeleton must be byte-identical across devices to dedupe"
    );

    // Each device: apply the shared skeleton, then splice its own content under its
    // own clientID.
    let device = |client_id: u64, content: &str| -> Vec<u8> {
        let doc = Doc::with_options(yrs_doc_options(document, client_id, OffsetKind::Utf16));
        doc.transact_mut()
            .apply_update(Update::decode_v1(&skeleton()).unwrap())
            .unwrap();
        {
            let items = doc.get_or_insert_map("items_by_id");
            let mut txn = doc.transact_mut();
            let Some(Out::YMap(item_map)) = items.get(&txn, item_id) else {
                panic!("skeleton item missing");
            };
            let Some(Out::YText(text)) = item_map.get(&txn, "text") else {
                panic!("skeleton text missing");
            };
            text.insert(&mut txn, 0, content);
        }
        let update = doc.transact().encode_diff_v1(&StateVector::default());
        update
    };
    let a = device(111, "hello");
    let b = device(222, "world");

    let merged = Doc::with_options(yrs_doc_options(document, 333, OffsetKind::Utf16));
    merged
        .transact_mut()
        .apply_update(Update::decode_v1(&a).unwrap())
        .unwrap();
    merged
        .transact_mut()
        .apply_update(Update::decode_v1(&b).unwrap())
        .unwrap();

    let items = merged.get_or_insert_map("items_by_id");
    let txn = merged.transact();
    assert_eq!(
        items.len(&txn),
        1,
        "concurrent creates deduped to ONE container"
    );
    let Some(Out::YMap(item_map)) = items.get(&txn, item_id) else {
        panic!("item missing after merge");
    };
    assert_eq!(
        item_map
            .get_as::<_, Option<String>>(&txn, "schema")
            .unwrap()
            .as_deref(),
        Some("knotq.item.v1"),
        "schema survived (no clobber)"
    );
    let Some(Out::YText(text)) = item_map.get(&txn, "text") else {
        panic!("text missing after merge");
    };
    let merged_text = text.get_string(&txn);
    assert!(
        merged_text.contains("hello") && merged_text.contains("world"),
        "both devices' concurrent content preserved (AB/BA), got {merged_text:?}"
    );
}

/// Bug A, through the real production path: two devices that independently create a
/// scheme containing the SAME item id (e.g. a carryover "today" item) via
/// `replace_scheme` must merge into a structurally valid document — the
/// deterministic skeleton dedupes the item container instead of clobbering one and
/// dropping its fields (which would surface as `item schema/position missing`).
#[test]
fn replace_scheme_dedupes_concurrent_same_item_creation() {
    let document = DocumentId::new();
    // One item value cloned to both devices, so they share the item id.
    let shared = Item::new("base");
    let make = |text: &str| {
        let mut item = shared.clone();
        item.set_text(text);
        let mut scheme = Scheme::new("Daily", 0);
        scheme.items.push(item);
        YrsSchemeDocument::from_scheme(document, &scheme).unwrap()
    };
    let a = make("alpha");
    let b = make("beta");

    let merged = YrsSchemeDocument::new(document);
    merged.apply_update_v1(&a.encode_state_v1()).unwrap();
    merged.apply_update_v1(&b.encode_state_v1()).unwrap();

    // No clobber: the merged item still has schema/id/position/text → validates.
    let state = merged.encode_state_v1();
    validate_crdt_update_sequence(SyncDocumentKind::Scheme, [state.as_slice()])
        .expect("concurrent same-item creation must merge to a valid document");
}

/// The server broadcasts `changed` to every device — including the one whose
/// push caused it — so each client re-pulls its own update. That echo must
/// merge as a reported no-op (`apply_update_v1 -> false`) so callers don't
/// treat it as a remote change and rebuild/reload the document the user is
/// actively editing.
#[test]
fn reapplying_an_already_merged_update_reports_no_change() {
    let document = DocumentId::new();
    let mut scheme = Scheme::new("Plan", 0);
    scheme.items.push(Item::new("First"));
    let left = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();

    let right = YrsSchemeDocument::new(document);
    let update = left.encode_update_v1(&right.state_vector_v1()).unwrap();
    assert!(
        right.apply_update_v1(&update).unwrap(),
        "first merge introduces new state"
    );
    assert!(
        !right.apply_update_v1(&update).unwrap(),
        "echo of an already-merged update is a no-op"
    );
    // The origin replica already holds everything it encoded.
    assert!(
        !left.apply_update_v1(&update).unwrap(),
        "a replica's own update echoed back is a no-op"
    );
}

/// Item removal syncs as a tombstone insert plus (in races) hard map removes,
/// whose effects can live partly in the delete set rather than the state
/// vector — the changed-detection must not mistake a deletion delta for an
/// echo.
#[test]
fn delete_only_update_reports_a_change() {
    let document = DocumentId::new();
    let mut base = Scheme::new("Plan", 0);
    base.items.push(Item::new("First"));
    base.items.push(Item::new("Second"));

    let left = YrsSchemeDocument::from_scheme(document, &base).unwrap();
    let right = YrsSchemeDocument::new(document);
    right
        .apply_update_v1(&left.encode_update_v1(&[]).unwrap())
        .unwrap();
    let synced_state = right.state_vector_v1();

    // Left deletes an item; the delta to an in-sync peer is delete-dominated.
    let mut edited = base.clone();
    edited.items.remove(1);
    left.sync_scheme(&edited).unwrap();
    let delta = left.encode_update_v1(&synced_state).unwrap();

    assert!(
        right.apply_update_v1(&delta).unwrap(),
        "a deletion must count as a change"
    );
    // The tombstone arrived: the deleted item is gone from materialization.
    let texts: Vec<String> = right
        .scheme_items()
        .unwrap()
        .iter()
        .map(|item| item.text())
        .collect();
    assert_eq!(texts, vec!["First"]);
    // Re-applying the same deletion is an echo again.
    assert!(!right.apply_update_v1(&delta).unwrap());
}

/// Re-adding an item after a soft-delete with byte-identical snapshot+position
/// (undo of a delete) must clear the tombstone in the DOC, not just the local
/// workspace. The metadata write is what sets `deleted=false`; skipping it as
/// "unchanged" leaves every peer (and this doc's own materialization)
/// considering the item deleted while the local workspace shows it alive.
#[test]
fn readding_a_tombstoned_item_untombstones_it() {
    let document = DocumentId::new();
    let mut scheme = Scheme::new("Plan", 0);
    scheme.items.push(Item::new("First"));
    scheme.items.push(Item::new("Second"));

    let doc = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();
    let peer = YrsSchemeDocument::new(document);
    peer.apply_update_v1(&doc.encode_update_v1(&[]).unwrap())
        .unwrap();

    // Delete "Second" (tombstone), then undo — the identical item comes back.
    let mut without = scheme.clone();
    without.items.remove(1);
    doc.sync_scheme(&without).unwrap();
    let readd = doc.sync_scheme(&scheme).unwrap();

    let texts = |d: &YrsSchemeDocument| -> Vec<String> {
        d.scheme_items()
            .unwrap()
            .iter()
            .map(|item| item.text())
            .collect()
    };
    assert_eq!(
        texts(&doc),
        vec!["First", "Second"],
        "the un-delete must stick in this doc's own materialization"
    );

    // And it must reach peers: the re-add emits an update carrying the un-delete.
    assert!(
        readd.is_some(),
        "re-adding a tombstoned item must emit an update"
    );
    peer.apply_update_v1(&doc.encode_update_v1(&peer.state_vector_v1()).unwrap())
        .unwrap();
    assert_eq!(texts(&peer), vec!["First", "Second"]);
}
