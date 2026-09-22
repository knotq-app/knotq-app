//! The first pull after an account switch: nothing the account being left
//! removed may reach the rows the destination account still has.
//!
//! Two accounts hold byte-identical structs for the same derived document id —
//! every install populates a Daily page's starter rows the same way, and a
//! device that switched before carried its very structs across — so a removal
//! authored on one account observes a row that is still live on the other.
//! Merged the ordinary way and re-seeded as a full snapshot, that removal
//! deleted the row for every device on the destination (production fuzz chaos
//! seeds 220 and 287: rolling a Daily row forward on one account emptied that
//! day on the other).
use super::super::*;

use knotq_model::{Item, ItemId, SchemeId};
use yrs::GetString;

/// Fixed ids, like the starter Daily rows: every install on every account
/// populates this exact content and therefore the exact same structs.
fn starter() -> Scheme {
    let mut scheme = Scheme::new("Daily 2026-09-14", 0);
    scheme.id = "00000000-0000-8000-8000-000000000201".parse().unwrap();
    for (index, text) in ["Plan the day", "Review inbox", "Wrap up"]
        .into_iter()
        .enumerate()
    {
        let mut item = Item::new(text);
        item.id = format!("00000000-0000-8000-8000-00000000040{}", index + 1)
            .parse::<ItemId>()
            .unwrap();
        scheme.items.push(item);
    }
    scheme
}

fn ids(doc: &YrsSchemeDocument) -> Vec<ItemId> {
    doc.scheme_items()
        .unwrap()
        .iter()
        .map(|item| item.id)
        .collect()
}

fn texts(doc: &YrsSchemeDocument) -> Vec<String> {
    doc.scheme_items()
        .unwrap()
        .iter()
        .map(|item| item.text())
        .collect()
}

fn all_changes(base: SchemeId, other: SchemeId) -> WorkspaceCrdtChangeSet {
    WorkspaceCrdtChangeSet::default()
        .workspace()
        .touch_scheme(base)
        .touch_scheme(other)
}

fn documents_for(base: &Scheme, folder_scheme: &Scheme) -> (Workspace, WorkspaceCrdtDocuments) {
    let mut workspace = Workspace::new();
    for scheme in [base, folder_scheme] {
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(knotq_model::NodeRef::Scheme(scheme.id));
        workspace.schemes.insert(scheme.id, scheme.clone());
    }
    workspace.ensure_sync_metadata();
    let docs = WorkspaceCrdtDocuments::from_states::<Vec<u8>>(
        &workspace,
        knotq_model::ReplicaId::new(),
        &HashMap::new(),
    )
    .unwrap();
    let mut docs = docs;
    let outcome = docs.sync_changes(&workspace, &all_changes(base.id, folder_scheme.id));
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    (workspace, docs)
}

fn scheme_doc(docs: &WorkspaceCrdtDocuments, scheme: SchemeId) -> &YrsSchemeDocument {
    docs.schemes.get(&scheme).unwrap()
}

/// Apply the destination's full state to `docs` the way the switch's pull
/// does, then run the revival pass, returning the merged workspace.
fn switch_into(
    workspace: &Workspace,
    docs: &mut WorkspaceCrdtDocuments,
    destination: &WorkspaceCrdtDocuments,
) -> Workspace {
    let states = destination.document_states();
    let updates: Vec<StoredCrdtUpdate> = workspace
        .scheme_sync
        .values()
        .filter_map(|meta| {
            states.get(&meta.id).map(|state| StoredCrdtUpdate {
                workspace_id: workspace.id,
                document: meta.id,
                kind: SyncDocumentKind::Scheme,
                replica_id: knotq_model::ReplicaId::new(),
                sequence: 1,
                received_at: chrono::Utc::now(),
                update_v1: state.to_vec(),
            })
        })
        .collect();
    let switched: HashSet<DocumentId> = updates.iter().map(|update| update.document).collect();
    let outcome = docs.apply_remote_updates_for_account_switch(workspace, &updates, &switched);
    assert!(
        outcome.is_ok(),
        "{:?} {:?}",
        outcome.document_errors,
        outcome.workspace_errors
    );
    let (merged, _) = docs
        .revive_after_account_switch(&outcome.workspace, &outcome.account_switch_merges)
        .unwrap();
    merged
}

fn items_of(workspace: &Workspace, scheme: SchemeId) -> Vec<ItemId> {
    workspace.schemes[&scheme]
        .items
        .iter()
        .map(|item| item.id)
        .collect()
}

#[test]
fn the_account_left_cannot_remove_a_row_the_destination_still_has() {
    let base = starter();
    let mut other = Scheme::new("Notes", 0);
    other.id = "00000000-0000-8000-8000-000000000202".parse().unwrap();
    let (workspace, mut device) = documents_for(&base, &other);
    let (_, mut destination) = documents_for(&base, &other);
    assert_eq!(
        scheme_doc(&device, base.id).encode_state_v1(),
        scheme_doc(&destination, base.id).encode_state_v1(),
        "both accounts hold identical structs"
    );

    // On the account being left this device removed the first row outright,
    // edited the second row's text, typed a line only it has, and typed then
    // removed another — a removal of its own that must survive the switch.
    let mut edited = workspace.clone();
    let scheme = edited.schemes.get_mut(&base.id).unwrap();
    let removed_here = scheme.items.remove(0).id;
    scheme.items[0].set_text("Review inbox now");
    let own_line = Item::new("only on this device");
    let own_line_id = own_line.id;
    scheme.items.push(own_line);
    let transient = Item::new("typed and removed before the switch");
    let transient_id = transient.id;
    scheme.items.push(transient);
    let changes = all_changes(base.id, other.id);
    assert!(device.sync_changes(&edited, &changes).errors.is_empty());
    edited.schemes.get_mut(&base.id).unwrap().items.pop();
    // An ordinary scheme write preserves raw-only copies; naming the id is
    // what a local delete does to tombstone one.
    let mut removal = all_changes(base.id, other.id);
    removal
        .deleted_items
        .entry(base.id)
        .or_default()
        .insert(transient_id.to_string());
    assert!(device.sync_changes(&edited, &removal).errors.is_empty());
    // Meanwhile the destination removed its last starter row.
    let mut destination_edit = workspace.clone();
    let removed_there = destination_edit
        .schemes
        .get_mut(&base.id)
        .unwrap()
        .items
        .remove(2)
        .id;
    let mut destination_removal = all_changes(base.id, other.id);
    destination_removal
        .deleted_items
        .entry(base.id)
        .or_default()
        .insert(removed_there.to_string());
    assert!(destination
        .sync_changes(&destination_edit, &destination_removal)
        .errors
        .is_empty());

    let merged = switch_into(&edited, &mut device, &destination);
    let merged_ids = items_of(&merged, base.id);
    assert!(
        merged_ids.contains(&removed_here),
        "the destination's live row survives the switch"
    );
    assert!(
        merged_ids.contains(&own_line_id),
        "the device's own line follows it"
    );
    assert!(
        !merged_ids.contains(&transient_id),
        "the device's own removal of its own line is kept"
    );
    assert!(
        !merged_ids.contains(&removed_there),
        "the destination's removal is honoured"
    );
    assert_eq!(
        merged.schemes[&base.id]
            .items
            .iter()
            .map(|item| item.text())
            .collect::<Vec<_>>(),
        ["Plan the day", "Review inbox now", "only on this device"],
        "text edits still cross, and shared structs merge rather than double"
    );

    // The re-seed the switch pushes is this document's full state. On a server
    // holding the destination's history it adds the device's line and edit
    // without removing the destination's row, and a device that only ever saw
    // the destination converges to the same thing.
    let device_state = scheme_doc(&device, base.id).encode_state_v1();
    let destination_doc = scheme_doc(&destination, base.id);
    let server = YrsSchemeDocument::new(destination_doc.id);
    server
        .apply_update_v1(&destination_doc.encode_state_v1())
        .unwrap();
    for other in [&server, destination_doc] {
        other.apply_update_v1(&device_state).unwrap();
        assert_eq!(ids(other), merged_ids);
        assert_eq!(texts(other), texts(scheme_doc(&device, base.id)));
    }
}

/// A row this device moved on the account it left is a move here too: the
/// destination's copy on the source scheme goes, the row lives where the
/// device put it, and nothing is duplicated for the placement reconciler to
/// delete again.
#[test]
fn a_row_the_device_moved_follows_the_move_instead_of_doubling() {
    let base = starter();
    let mut other = Scheme::new("Notes", 0);
    other.id = "00000000-0000-8000-8000-000000000202".parse().unwrap();
    let (workspace, mut device) = documents_for(&base, &other);
    let (_, destination) = documents_for(&base, &other);

    let mut moved = workspace.clone();
    let row = moved.schemes.get_mut(&base.id).unwrap().items.remove(0);
    let row_id = row.id;
    moved.schemes.get_mut(&other.id).unwrap().items.push(row);
    let mut changes = all_changes(base.id, other.id);
    changes
        .deleted_items
        .entry(base.id)
        .or_default()
        .insert(row_id.to_string());
    assert!(device.sync_changes(&moved, &changes).errors.is_empty());

    let merged = switch_into(&moved, &mut device, &destination);
    assert!(!items_of(&merged, base.id).contains(&row_id));
    assert_eq!(
        items_of(&merged, other.id),
        [row_id],
        "the row lives exactly where the device moved it"
    );
    assert_eq!(device.documents_holding_item(row_id), [other.id]);
}

/// The two-sided case the fuzzer found: the device moved the row on the
/// account it left, and the destination meanwhile removed the copy on the
/// target scheme (as a cross-document duplicate, say). Each removal alone is
/// a move; together they leave the row dead everywhere, so it comes back
/// where the device had it.
#[test]
fn a_row_removed_on_both_sides_comes_back_where_the_device_held_it() {
    let base = starter();
    let mut other = Scheme::new("Notes", 0);
    other.id = "00000000-0000-8000-8000-000000000202".parse().unwrap();
    let (workspace, mut device) = documents_for(&base, &other);
    let (_, mut destination) = documents_for(&base, &other);

    // The destination once had the row on both schemes and removed the copy
    // on `other` — the same structs the device's move creates there.
    let mut doubled = workspace.clone();
    let row = doubled.schemes[&base.id].items[0].clone();
    let row_id = row.id;
    doubled
        .schemes
        .get_mut(&other.id)
        .unwrap()
        .items
        .push(row.clone());
    let changes = all_changes(base.id, other.id);
    assert!(destination
        .sync_changes(&doubled, &changes)
        .errors
        .is_empty());
    let mut deduped = workspace.clone();
    let mut dedupe = all_changes(base.id, other.id);
    dedupe
        .deleted_items
        .entry(other.id)
        .or_default()
        .insert(row_id.to_string());
    let _ = &mut deduped;
    assert!(destination
        .sync_changes(&deduped, &dedupe)
        .errors
        .is_empty());
    assert_eq!(destination.documents_holding_item(row_id), [base.id]);

    let mut moved = workspace.clone();
    let row = moved.schemes.get_mut(&base.id).unwrap().items.remove(0);
    moved.schemes.get_mut(&other.id).unwrap().items.push(row);
    let mut move_changes = all_changes(base.id, other.id);
    move_changes
        .deleted_items
        .entry(base.id)
        .or_default()
        .insert(row_id.to_string());
    assert!(device.sync_changes(&moved, &move_changes).errors.is_empty());

    // Merged the ordinary way, the row is gone from both schemes.
    let mut plain = WorkspaceCrdtDocuments::from_states(
        &moved,
        knotq_model::ReplicaId::new(),
        &device.document_states(),
    )
    .unwrap();
    for (scheme, state) in [
        (base.id, scheme_doc(&destination, base.id).encode_state_v1()),
        (
            other.id,
            scheme_doc(&destination, other.id).encode_state_v1(),
        ),
    ] {
        scheme_doc(&plain, scheme).apply_update_v1(&state).unwrap();
    }
    assert!(plain.documents_holding_item(row_id).is_empty());
    let _ = &mut plain;

    let merged = switch_into(&moved, &mut device, &destination);
    assert!(!items_of(&merged, base.id).contains(&row_id));
    assert_eq!(items_of(&merged, other.id), [row_id]);
    assert_eq!(device.documents_holding_item(row_id), [other.id]);
}

#[test]
fn narrowing_keeps_every_struct_and_only_tombstones_the_receiver_lacks() {
    let document = DocumentId::new();
    // Sender and receiver author under the same clientID, so their structs
    // alias exactly as two accounts' identical populations do.
    let sender = Doc::with_options(yrs_doc_options(document, 7, OffsetKind::Utf16));
    let text = sender.get_or_insert_text("body");
    text.insert(&mut sender.transact_mut(), 0, "hello world");
    let known = sender.transact().state_vector();
    // A deletion of a struct `known` covers, and one of a struct past it.
    text.remove_range(&mut sender.transact_mut(), 5, 6);
    text.insert(&mut sender.transact_mut(), 5, "!!");
    text.remove_range(&mut sender.transact_mut(), 5, 2);
    let full = sender.transact().encode_diff_v1(&StateVector::default());

    let narrowed =
        super::super::encoding::update_v1_without_deletes_known_to(&full, &known).unwrap();
    let full_update = Update::decode_v1(&full).unwrap();
    let narrowed_update = Update::decode_v1(&narrowed).unwrap();
    assert_eq!(
        narrowed_update.state_vector(),
        full_update.state_vector(),
        "every struct is kept"
    );
    let mut kept = 0;
    for (client, ranges) in narrowed_update.delete_set().iter() {
        for range in ranges.iter() {
            assert!(
                range.start >= known.get(client),
                "a tombstone for a struct the receiver holds was kept: {client:?} {range:?}"
            );
            kept += range.end - range.start;
        }
    }
    assert_eq!(
        kept, 2,
        "the deletion of the sender's own later struct is carried"
    );

    // Over a receiver holding the known structs live, the narrowed update
    // leaves them alone while the full one deletes " world".
    for (update, expected) in [(narrowed, "hello world"), (full, "hello")] {
        let receiver = Doc::with_options(yrs_doc_options(document, 7, OffsetKind::Utf16));
        let body = receiver.get_or_insert_text("body");
        body.insert(&mut receiver.transact_mut(), 0, "hello world");
        receiver
            .transact_mut()
            .apply_update(Update::decode_v1(&update).unwrap())
            .unwrap();
        assert_eq!(body.get_string(&receiver.transact()), expected);
    }
}
