//! Unit tests for the CRDT layer. Kept in their own file but still a child of the
//! `crdt` module, so `use super::*` reaches the same internal items as before.
use super::*;

    use knotq_model::{
        CalendarProvider, ImageAssetFormat, ImageInline, ImportedCalendarSource, Item, NodeRef,
    };

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

        validate_crdt_update_sequence(SyncDocumentKind::PersonalWorkspace, workspace_updates)
            .unwrap();
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

        assert!(
            validate_crdt_update_sequence(SyncDocumentKind::Scheme, [delta.as_slice()]).is_err()
        );
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
        let item_map =
            items_by_id.insert(&mut txn, ItemId::new().to_string(), MapPrelim::default());
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
        write_new_item(&kept_map, &mut txn, &kept, "V", &item_snapshot_json(&kept).unwrap())
            .unwrap();
        let partial_map =
            items_by_id.insert(&mut txn, partial.id.to_string(), MapPrelim::default());
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

    #[test]
    fn crdt_schema_validation_rejects_folder_documents() {
        let doc = Doc::new();
        assert!(validate_crdt_update_sequence(
            SyncDocumentKind::Folder,
            [encode_full_update(&doc).as_slice()]
        )
        .is_err());
    }

    #[test]
    fn workspace_crdt_documents_emit_scheme_updates_for_touched_schemes() {
        let mut workspace = Workspace::new();
        let mut scheme = Scheme::new("Plan", 0);
        scheme.items.push(Item::new("First"));
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.ensure_sync_metadata();

        let mut docs = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        workspace.schemes.get_mut(&scheme_id).unwrap().items[0].set_text("Changed");
        let updates = docs
            .sync_changes(
                &workspace,
                &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
            )
            .updates;

        assert!(updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::Scheme));
    }

    #[test]
    fn remote_crdt_updates_materialize_workspace_and_scheme_items() {
        let mut source = Workspace::new();
        let mut scheme = Scheme::new("Remote Plan", 2);
        scheme.items.push(Item::new("First remote line"));
        let scheme_id = scheme.id;
        source
            .folders
            .get_mut(&source.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme_id));
        source.schemes.insert(scheme_id, scheme);
        source.ensure_sync_metadata();

        let updates = WorkspaceCrdtDocuments::snapshot_updates(&source)
            .updates
            .into_iter()
            .enumerate()
            .map(|(index, update)| StoredCrdtUpdate {
                workspace_id: source.id,
                document: update.document,
                kind: update.kind,
                replica_id: knotq_model::ReplicaId::new(),
                sequence: (index + 1) as u64,
                received_at: chrono::Utc::now(),
                update_v1: update.update_v1,
            })
            .collect::<Vec<_>>();

        let mut target = source.clone();
        target.schemes.get_mut(&scheme_id).unwrap().items.clear();
        let mut docs = WorkspaceCrdtDocuments::try_new(&target).unwrap();
        let outcome = docs.apply_remote_updates(&target, &updates);

        assert!(outcome.is_ok(), "{:?}", outcome.document_errors);
        assert_eq!(
            outcome.workspace.schemes[&scheme_id].items[0].text(),
            "First remote line"
        );
        assert!(outcome.workspace.folders[&outcome.workspace.root]
            .children
            .contains(&NodeRef::Scheme(scheme_id)));
    }

    #[test]
    fn reidentified_workspace_merges_local_and_server_schemes_without_mismatch() {
        // The server account's canonical personal-workspace document (id B) holds
        // its own scheme. Mirror the server-side canonicalization so its document
        // id is DocumentId(workspace_id).
        let mut server = Workspace::new();
        let server_scheme = add_root_scheme(&mut server, "ServerScheme");
        server.canonicalize_personal_sync_identity(server.id);
        let server_doc_id = server.sync.id;
        let server_workspace_update: Vec<CrdtDocumentUpdate> =
            WorkspaceCrdtDocuments::snapshot_updates(&server)
                .updates
                .into_iter()
                .filter(|update| update.kind == SyncDocumentKind::PersonalWorkspace)
                .collect();

        // This device's workspace (a different account, id A) holds its own scheme.
        let mut local = Workspace::new();
        let local_scheme = add_root_scheme(&mut local, "LocalScheme");
        local.canonicalize_personal_sync_identity(local.id);
        assert_ne!(local.sync.id, server_doc_id);

        // Applying the server's workspace update as-is fails with a document-id
        // mismatch (the bug): the local CRDT workspace doc still carries id A.
        let mut before = WorkspaceCrdtDocuments::try_new(&local).unwrap();
        let before_outcome = before
            .apply_remote_updates(&local, &stored_updates(server.id, server_workspace_update.clone()));
        assert!(
            !before_outcome.workspace_is_ok(),
            "expected a document-id mismatch before re-identify"
        );

        // Re-identify the local workspace doc to the server's id (preserving its
        // content) and adopt the server account identity on the materialized
        // workspace, then the same update applies cleanly and unions both schemes.
        let mut docs = WorkspaceCrdtDocuments::try_new(&local).unwrap();
        let snapshot = docs.reidentify_workspace_document(server_doc_id).unwrap();
        assert!(snapshot.is_some(), "re-identify should report the relabeled doc");
        local.canonicalize_personal_sync_identity(server.id);
        let outcome =
            docs.apply_remote_updates(&local, &stored_updates(server.id, server_workspace_update));
        assert!(outcome.workspace_is_ok(), "{:?}", outcome.workspace_errors);
        assert!(
            outcome.workspace.schemes.contains_key(&local_scheme),
            "local scheme preserved through the merge"
        );
        assert!(
            outcome.workspace.schemes.contains_key(&server_scheme),
            "server scheme merged in over the shared id"
        );
    }

    #[test]
    fn remote_workspace_materialization_keeps_trash_and_daily_queue_out_of_sidebar() {
        let mut source = Workspace::new();
        let active = add_root_scheme(&mut source, "Active");

        let archived = Scheme::new("Archived", 1);
        let archived_id = archived.id;
        source.schemes.insert(archived_id, archived);
        source.mark_scheme_deleted_from(archived_id, source.root, 0);

        let daily_date = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let daily_id = knotq_model::daily_queue_scheme_id(daily_date);
        let mut daily = Scheme::new("Daily", 0);
        daily.id = daily_id;
        source.daily_queue.insert(daily_date, daily_id);
        source.schemes.insert(daily_id, daily);
        source.ensure_sync_metadata();

        let updates = WorkspaceCrdtDocuments::snapshot_updates(&source).updates;
        let mut target = Workspace::new();
        target.id = source.id;
        target.sync = source.sync.clone();
        target.root = source.root;
        target.folders.insert(
            source.root,
            Folder {
                id: source.root,
                name: "root".to_string(),
                parent: None,
                children: Vec::new(),
                expanded: true,
            },
        );
        let mut docs = WorkspaceCrdtDocuments::empty(&target);
        let outcome = docs.apply_remote_updates(&target, &stored_updates(source.id, updates));

        assert!(outcome.is_ok(), "{:?}", outcome.document_errors);
        let root_children = &outcome.workspace.folders[&outcome.workspace.root].children;
        assert!(root_children.contains(&NodeRef::Scheme(active)));
        assert!(!root_children.contains(&NodeRef::Scheme(archived_id)));
        assert!(!root_children.contains(&NodeRef::Scheme(daily_id)));
        assert!(outcome.workspace.is_scheme_deleted(archived_id));
        assert_eq!(
            outcome.workspace.daily_queue_scheme_id(daily_date),
            Some(daily_id)
        );
    }

    #[test]
    fn incremental_archive_update_keeps_scheme_out_of_sidebar() {
        let mut source = Workspace::new();
        let scheme = add_root_scheme(&mut source, "Archive Me");
        // One persistent document produces both the initial snapshot and the
        // later archive delta, exactly as the client's long-lived Store does.
        // Emitting them from two different documents would give the same logical
        // state two different Yjs identities and break LWW convergence.
        let mut source_docs = WorkspaceCrdtDocuments::empty(&source);
        let initial_updates = source_docs
            .sync_changes(&source, &WorkspaceCrdtChangeSet::default().workspace())
            .updates;

        let mut target = Workspace::new();
        target.id = source.id;
        target.sync = source.sync.clone();
        target.root = source.root;
        let mut target_docs = WorkspaceCrdtDocuments::empty(&target);
        let initial =
            target_docs.apply_remote_updates(&target, &stored_updates(source.id, initial_updates));
        assert!(initial.is_ok(), "{:?}", initial.document_errors);
        target = initial.workspace;
        assert!(target
            .folder(target.root)
            .unwrap()
            .children
            .contains(&NodeRef::Scheme(scheme)));

        source
            .folders
            .get_mut(&source.root)
            .unwrap()
            .children
            .retain(|child| *child != NodeRef::Scheme(scheme));
        source.mark_scheme_deleted_from(scheme, source.root, 0);
        let archive_updates = source_docs
            .sync_changes(&source, &WorkspaceCrdtChangeSet::default().workspace())
            .updates;
        assert!(
            !archive_updates.is_empty(),
            "archive should emit a workspace update"
        );

        let archived =
            target_docs.apply_remote_updates(&target, &stored_updates(source.id, archive_updates));
        assert!(archived.is_ok(), "{:?}", archived.document_errors);
        assert!(archived.workspace.is_scheme_deleted(scheme));
        assert!(!archived
            .workspace
            .folder(archived.workspace.root)
            .unwrap()
            .children
            .contains(&NodeRef::Scheme(scheme)));
    }

    #[test]
    fn workspace_crdt_documents_emit_workspace_updates_for_removed_schemes() {
        let mut workspace = Workspace::new();
        let scheme = Scheme::new("Plan", 0);
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace.mark_scheme_deleted(scheme_id);
        workspace.ensure_sync_metadata();

        let mut docs = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
        workspace.schemes.remove(&scheme_id);
        workspace.recently_deleted.retain(|id| *id != scheme_id);
        workspace.ensure_sync_metadata();

        let updates = docs
            .sync_changes(
                &workspace,
                &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
            )
            .updates;

        assert!(updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::PersonalWorkspace));
    }

    #[test]
    fn folder_changes_emit_workspace_document_not_folder_documents() {
        let mut workspace = Workspace::new();
        let folder = Folder {
            id: FolderId::new(),
            name: "Projects".to_string(),
            parent: Some(workspace.root),
            children: Vec::new(),
            expanded: true,
        };
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(folder.id));
        workspace.folders.insert(folder.id, folder);
        workspace.ensure_sync_metadata();

        let mut docs = WorkspaceCrdtDocuments::empty(&workspace);
        let updates = docs
            .sync_changes(&workspace, &WorkspaceCrdtChangeSet::default().workspace())
            .updates;

        assert!(updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::PersonalWorkspace));
        assert!(!updates
            .iter()
            .any(|update| update.kind == SyncDocumentKind::Folder));
    }

    #[test]
    fn concurrent_folder_additions_on_two_replicas_merge_without_loss() {
        // A shared base workspace that both replicas start from.
        let base = Workspace::new();

        // Each replica adds a different folder under the root and pushes its full
        // document state, exactly as a first-time/bootstrap sync does.
        let mut workspace_a = base.clone();
        let folder_x = add_root_folder(&mut workspace_a, "X");
        let a_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_a).updates;

        let mut workspace_b = base.clone();
        let folder_y = add_root_folder(&mut workspace_b, "Y");
        let b_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_b).updates;

        // The server holds the base and merges both replicas' deltas.
        let mut server = WorkspaceCrdtDocuments::try_new(&base).unwrap();
        let outcome_a = server.apply_remote_updates(&base, &stored_updates(base.id, a_updates));
        assert!(outcome_a.is_ok(), "{:?}", outcome_a.document_errors);
        let outcome_b =
            server.apply_remote_updates(&outcome_a.workspace, &stored_updates(base.id, b_updates));
        assert!(outcome_b.is_ok(), "{:?}", outcome_b.document_errors);

        // Both concurrently-added folders survive — neither clobbers the other the
        // way a single whole-document last-writer-wins blob would.
        let merged = outcome_b.workspace;
        assert!(merged.folders.contains_key(&folder_x), "folder X lost");
        assert!(merged.folders.contains_key(&folder_y), "folder Y lost");
        let root_children = &merged.folders[&merged.root].children;
        assert!(root_children.contains(&NodeRef::Folder(folder_x)));
        assert!(root_children.contains(&NodeRef::Folder(folder_y)));
    }

    #[test]
    fn concurrent_scheme_additions_under_root_merge_without_loss() {
        let base = Workspace::new();

        let mut workspace_a = base.clone();
        let scheme_a = add_root_scheme(&mut workspace_a, "A");
        let a_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_a).updates;

        let mut workspace_b = base.clone();
        let scheme_b = add_root_scheme(&mut workspace_b, "B");
        let b_updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace_b).updates;

        let mut server = WorkspaceCrdtDocuments::try_new(&base).unwrap();
        let outcome_a = server.apply_remote_updates(&base, &stored_updates(base.id, a_updates));
        assert!(outcome_a.is_ok(), "{:?}", outcome_a.document_errors);
        let outcome_b =
            server.apply_remote_updates(&outcome_a.workspace, &stored_updates(base.id, b_updates));
        assert!(outcome_b.is_ok(), "{:?}", outcome_b.document_errors);

        let merged = outcome_b.workspace;
        assert!(merged.schemes.contains_key(&scheme_a), "scheme A lost");
        assert!(merged.schemes.contains_key(&scheme_b), "scheme B lost");
        let root_children = &merged.folders[&merged.root].children;
        assert!(root_children.contains(&NodeRef::Scheme(scheme_a)));
        assert!(root_children.contains(&NodeRef::Scheme(scheme_b)));
    }

    fn add_root_folder(workspace: &mut Workspace, name: &str) -> FolderId {
        let folder = Folder {
            id: FolderId::new(),
            name: name.to_string(),
            parent: Some(workspace.root),
            children: Vec::new(),
            expanded: true,
        };
        let id = folder.id;
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Folder(id));
        workspace.folders.insert(id, folder);
        workspace.ensure_sync_metadata();
        id
    }

    fn add_root_scheme(workspace: &mut Workspace, name: &str) -> SchemeId {
        let scheme = Scheme::new(name, 0);
        let id = scheme.id;
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(id));
        workspace.schemes.insert(id, scheme);
        workspace.ensure_sync_metadata();
        id
    }

    fn stored_updates(
        workspace_id: knotq_model::WorkspaceId,
        updates: Vec<CrdtDocumentUpdate>,
    ) -> Vec<StoredCrdtUpdate> {
        updates
            .into_iter()
            .enumerate()
            .map(|(index, update)| StoredCrdtUpdate {
                workspace_id,
                document: update.document,
                kind: update.kind,
                replica_id: knotq_model::ReplicaId::new(),
                sequence: (index + 1) as u64,
                received_at: chrono::Utc::now(),
                update_v1: update.update_v1,
            })
            .collect()
    }

    fn valid_single_item_scheme_doc() -> Doc {
        let doc = Doc::new();
        let metadata = doc.get_or_insert_map("scheme_file");
        let items_by_id = doc.get_or_insert_map("items_by_id");
        let scheme = Scheme::new("Plan", 0);
        let item = Item::new("First");
        let item_id = item.id.to_string();
        let mut txn = doc.transact_mut();
        metadata.insert(&mut txn, "schema", SCHEME_SCHEMA_V1);
        metadata.insert(&mut txn, "id", scheme.id.to_string());
        let item_map = items_by_id.insert(&mut txn, item_id, MapPrelim::default());
        let snapshot_json = item_snapshot_json(&item).unwrap();
        write_new_item(&item_map, &mut txn, &item, "V", &snapshot_json).unwrap();
        drop(txn);
        doc
    }

    fn encode_full_update(doc: &Doc) -> Vec<u8> {
        doc.transact().encode_diff_v1(&StateVector::default())
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
        assert_eq!(items.len(&txn), 1, "concurrent creates deduped to ONE container");
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
