//! CRDT schema-validation unit tests.
use super::super::*;

use super::helpers::{encode_full_update, valid_single_item_scheme_doc};
use knotq_model::{CalendarProvider, ImportedCalendarSource, Item, SchemeSource};

#[test]
fn crdt_schema_validation_accepts_workspace_snapshots() {
    let mut workspace = Workspace::new();
    let scheme = Scheme::new("Plan", 0);
    workspace.schemes.insert(scheme.id, scheme);
    workspace.ensure_sync_metadata();

    let mut docs = WorkspaceCrdtDocuments::empty(&workspace);
    let updates = docs
        .sync_changes(&workspace, &WorkspaceCrdtChangeSet::default().workspace())
        .updates;
    let workspace_updates = updates
        .iter()
        .filter(|update| update.kind == SyncDocumentKind::PersonalWorkspace)
        .map(|update| update.update_v1.as_slice());

    validate_crdt_update_sequence(SyncDocumentKind::PersonalWorkspace, workspace_updates).unwrap();
}

#[test]
fn workspace_crdt_snapshot_omits_google_calendar_sync_token() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Imported", 0);
    scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
        provider: CalendarProvider::Google,
        account_id: "account".to_string(),
        account_email: Some("user@example.com".to_string()),
        calendar_id: "calendar".to_string(),
        sync_token: Some("local-google-sync-token".to_string()),
        read_only: true,
        last_synced_at: None,
    });
    workspace.schemes.insert(scheme.id, scheme);
    workspace.ensure_sync_metadata();

    let snapshot = workspace_document_snapshot(&workspace);
    let SchemeSource::ImportedCalendar(source) = &snapshot.schemes[0].source else {
        panic!("expected imported calendar source");
    };
    assert_eq!(source.provider, CalendarProvider::Google);
    assert_eq!(source.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(source.sync_token, None);
    assert!(source.read_only);
}

#[test]
fn remote_workspace_materialization_preserves_local_google_calendar_sync_token() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Imported", 0);
    let scheme_id = scheme.id;
    scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
        provider: CalendarProvider::Google,
        account_id: "account".to_string(),
        account_email: Some("user@example.com".to_string()),
        calendar_id: "calendar".to_string(),
        sync_token: Some("local-token".to_string()),
        read_only: true,
        last_synced_at: None,
    });
    workspace.schemes.insert(scheme_id, scheme);

    let remote_source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
        provider: CalendarProvider::Google,
        account_id: "account".to_string(),
        account_email: Some("user@example.com".to_string()),
        calendar_id: "calendar".to_string(),
        sync_token: None,
        read_only: true,
        last_synced_at: None,
    });

    let SchemeSource::ImportedCalendar(merged) =
        preserve_local_calendar_sync_token(&workspace, scheme_id, remote_source)
    else {
        panic!("expected imported calendar source");
    };
    assert_eq!(merged.sync_token.as_deref(), Some("local-token"));
}

#[test]
fn crdt_schema_validation_accepts_scheme_history_and_delta() {
    let document = DocumentId::new();
    let mut scheme = Scheme::new("Plan", 0);
    scheme.items.push(Item::new("First"));
    let doc = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();
    let initial = doc.encode_update_v1(&[]).unwrap();

    scheme.items[0].set_text("Changed");
    let delta = doc.sync_scheme(&scheme).unwrap().unwrap().update_v1;

    validate_crdt_update_sequence(
        SyncDocumentKind::Scheme,
        [initial.as_slice(), delta.as_slice()],
    )
    .unwrap();
}

#[test]
fn crdt_schema_validation_rejects_malformed_update_bytes() {
    let err = validate_crdt_update_sequence(SyncDocumentKind::Scheme, [&[1, 2, 3][..]])
        .unwrap_err()
        .to_string();

    assert!(err.contains("decode update_v1"));
}

#[test]
fn crdt_schema_validation_rejects_delta_without_base_document() {
    let document = DocumentId::new();
    let mut scheme = Scheme::new("Plan", 0);
    scheme.items.push(Item::new("First"));
    let doc = YrsSchemeDocument::from_scheme(document, &scheme).unwrap();
    let _initial = doc.encode_update_v1(&[]).unwrap();

    scheme.items[0].set_text("Changed");
    let delta = doc.sync_scheme(&scheme).unwrap().unwrap().update_v1;

    assert!(validate_crdt_update_sequence(SyncDocumentKind::Scheme, [delta.as_slice()]).is_err());
}

#[test]
fn crdt_schema_validation_rejects_bad_workspace_schema() {
    let doc = Doc::new();
    let meta = doc.get_or_insert_map("meta");
    let mut txn = doc.transact_mut();
    meta.insert(&mut txn, "schema", "bad.workspace");
    meta.insert(&mut txn, "id", Workspace::new().id.to_string());
    meta.insert(&mut txn, "root", FolderId::new().to_string());
    meta.insert(&mut txn, "sync", "{}");
    drop(txn);

    assert!(validate_crdt_update_sequence(
        SyncDocumentKind::PersonalWorkspace,
        [encode_full_update(&doc).as_slice()]
    )
    .is_err());
}

#[test]
fn crdt_schema_validation_rejects_bad_scheme_schema() {
    let doc = valid_single_item_scheme_doc();
    let metadata = doc.get_or_insert_map("scheme_file");
    metadata.insert(&mut doc.transact_mut(), "schema", "bad.scheme");

    assert!(validate_crdt_update_sequence(
        SyncDocumentKind::Scheme,
        [encode_full_update(&doc).as_slice()]
    )
    .is_err());
}

#[test]
fn crdt_schema_validation_accepts_dotted_marker_subtype() {
    let doc = valid_single_item_scheme_doc();
    let items_by_id = doc.get_or_insert_map("items_by_id");
    let txn = doc.transact();
    let item_key = items_by_id.keys(&txn).next().unwrap().to_string();
    let item_map = item_map_ref(&items_by_id, &txn, &item_key).unwrap();
    drop(txn);
    item_map.insert(&mut doc.transact_mut(), "marker", "numbered.alphabet");

    validate_crdt_update_sequence(
        SyncDocumentKind::Scheme,
        [encode_full_update(&doc).as_slice()],
    )
    .unwrap();
}

#[test]
fn crdt_schema_validation_tolerates_item_without_position() {
    // A structurally-incomplete item (here: empty position) is tolerated, not
    // rejected: rejecting the whole document would wedge the push, and a wedged
    // update never propagates — the only way Yjs replicas permanently diverge. The
    // scheme-level structure is valid, so the document validates.
    let doc = Doc::new();
    let metadata = doc.get_or_insert_map("scheme_file");
    let items_by_id = doc.get_or_insert_map("items_by_id");
    let item = Item::new("First");
    let mut txn = doc.transact_mut();
    metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
    metadata.insert(&mut txn, "id", SchemeId::new().to_string());
    let item_map = items_by_id.insert(&mut txn, item.id.to_string(), MapPrelim::default());
    let snapshot_json = item_snapshot_json(&item).unwrap();
    write_new_item(&item_map, &mut txn, &item, "", &snapshot_json).unwrap();
    drop(txn);

    validate_crdt_update_sequence(
        SyncDocumentKind::Scheme,
        [encode_full_update(&doc).as_slice()],
    )
    .unwrap();
}

#[test]
fn crdt_schema_validation_tolerates_item_id_key_mismatch() {
    // An item whose map key disagrees with its stored id is tolerated rather than
    // rejected, for the same reason: a single partial item must never wedge the
    // whole document.
    let doc = Doc::new();
    let metadata = doc.get_or_insert_map("scheme_file");
    let items_by_id = doc.get_or_insert_map("items_by_id");
    let item = Item::new("First");
    let mut txn = doc.transact_mut();
    metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
    metadata.insert(&mut txn, "id", SchemeId::new().to_string());
    // Store the item under a different (still valid) key than its own id.
    let item_map = items_by_id.insert(&mut txn, ItemId::new().to_string(), MapPrelim::default());
    let snapshot_json = item_snapshot_json(&item).unwrap();
    write_new_item(&item_map, &mut txn, &item, "V", &snapshot_json).unwrap();
    drop(txn);

    validate_crdt_update_sequence(
        SyncDocumentKind::Scheme,
        [encode_full_update(&doc).as_slice()],
    )
    .unwrap();
}

#[test]
fn crdt_schema_validation_tolerates_schema_missing_partial_item() {
    // The exact multi-origin merge artifact that used to wedge sync: an item that
    // keeps its content and snapshot but loses its `schema` struct (concurrent
    // map-entry churn across origins can delete every copy of the field). The
    // strict per-item check flags it, but the document must still validate so the
    // push is not rejected — a rejected push wedges sync and never propagates,
    // which is the only way replicas permanently diverge. Other valid items in the
    // same document are unaffected, and materialization reads snapshot_json (not
    // this struct), so every replica still renders the kept item identically.
    let doc = Doc::new();
    let metadata = doc.get_or_insert_map("scheme_file");
    let items_by_id = doc.get_or_insert_map("items_by_id");
    let kept = Item::new("kept");
    let partial = Item::new("partial");
    let mut txn = doc.transact_mut();
    metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
    metadata.insert(&mut txn, "id", SchemeId::new().to_string());
    let kept_map = items_by_id.insert(&mut txn, kept.id.to_string(), MapPrelim::default());
    write_new_item(
        &kept_map,
        &mut txn,
        &kept,
        "V",
        &item_snapshot_json(&kept).unwrap(),
    )
    .unwrap();
    let partial_map = items_by_id.insert(&mut txn, partial.id.to_string(), MapPrelim::default());
    write_new_item(
        &partial_map,
        &mut txn,
        &partial,
        "W",
        &item_snapshot_json(&partial).unwrap(),
    )
    .unwrap();
    // Strand the schema struct, reproducing the cross-origin map-entry clobber.
    partial_map.remove(&mut txn, "schema");
    drop(txn);

    validate_crdt_update_sequence(
        SyncDocumentKind::Scheme,
        [encode_full_update(&doc).as_slice()],
    )
    .unwrap();
}

#[test]
fn crdt_schema_validation_rejects_folder_documents() {
    let doc = Doc::new();
    assert!(validate_crdt_update_sequence(
        SyncDocumentKind::Folder,
        [encode_full_update(&doc).as_slice()]
    )
    .is_err());
}
