//! Regression tests that reproduce the two production wedge bugs diagnosed from a
//! real client that got stuck with 688 pending CRDT edits and every push rejected
//! with `crdt_schema_invalid`.
//!
//! ## Bug 1 — Sequence reset on restart
//!
//! `desktop/state/src/store.rs:87` hard-codes `next_sequence: 1` at construction.
//! After an app restart, new pending edits reuse local_sequence 1, 2, 3 … while
//! *older, unpushed* edits with those same sequences are still in the persisted
//! `LocalSyncState.pending` (we observed exactly this in the real `sync-state.json`:
//! seq 1, 2, 3 then 1, 2, 3 again for the same document).
//! `LocalSyncState::mark_pushed(document, through_local_sequence)` clears by
//! `seq <= through`, so acknowledging the first push batch silently drops the
//! second batch's edits.  The server then has gaps in this client's Yjs clock ranges,
//! every later delta reconstructs an incomplete document, and the server rejects it
//! as `crdt_schema_invalid` forever.
//!
//! ## Bug 2 — No recovery path on `crdt_schema_invalid`
//!
//! When the server rejects a push, `batch_push_pending` returns `Err` and the sync
//! run aborts.  The next run retries the same bad deltas.  The only reseed logic
//! (`queue_workspace_bootstrap_updates`) reseeds a full snapshot only when the server
//! has NO base (remote seq 0); when the server has partial state, there is no heal.
//!
//! ## Test status
//!
//! All tests in this file are written to assert the **desired** end state.
//! They are expected to **fail today** (before the fixes land) on the *bug*, not on
//! harness mistakes.  `restart_with_pending_edits_must_not_wedge_sync` and
//! `mark_pushed_clears_only_sent_edits` demonstrate Bug 1;
//! `schema_invalid_rejection_must_self_heal` demonstrates Bug 2;
//! `image_media_syncs_between_devices` demonstrates media sync and its dependence on
//! CRDT correctness.

mod common;

use common::{Harness, D0, D1};

// ---------------------------------------------------------------------------
// Bug 1a — restart_with_pending_edits_must_not_wedge_sync
// ---------------------------------------------------------------------------

/// Demonstrates the data-loss path from Bug 1: when a device has MORE than
/// `PUSH_MAX_UPDATES_PER_DOCUMENT` (50) pending edits for a document, they are split
/// across two push batches.  A legacy sequence reset causes post-restart edits
/// (seq 1, 2, …) to share sequence numbers with the pre-restart edits.  After the
/// first batch is acknowledged, `mark_pushed(doc, N)` clears ALL seq <= N, which
/// silently drops the post-restart edits that were NOT in the first batch.  Their
/// content ("line AFTER-RESTART") therefore never reaches the server.
///
/// **Expected (desired) outcome**: all edits, including the post-restart ones, reach
/// the server; D1 sees the full content.
///
/// **Today's outcome**: the post-restart edits are silently dropped by
/// `mark_pushed`, so D1 never sees "line AFTER-RESTART".  The test fails on the
/// content convergence assertion.
#[test]
fn restart_with_pending_edits_must_not_wedge_sync() {
    use knotq_sync::PUSH_MAX_UPDATES_PER_DOCUMENT;

    let mut h = Harness::new(2);
    h.login_all();

    // --- Phase 1: fill the scheme doc's pending queue beyond the push batch cap --
    //
    // We need PUSH_MAX_UPDATES_PER_DOCUMENT (50) pre-restart pending edits for the
    // *scheme document* so that the first push batch only covers seq 1..50 and the
    // post-restart edits (which also get seq 1, 2) are left in the queue when
    // mark_pushed fires.

    let scheme = h.add_scheme(D0, "Wedge Plan", &["seed"]);

    // Add enough lines to overflow the per-document batch limit.
    for i in 0..PUSH_MAX_UPDATES_PER_DOCUMENT {
        h.append_line(D0, scheme, &format!("pre-restart line {i}"));
    }

    // All of these should be in pending (no sync yet).
    let pre_restart_count = h.device(D0).pending_count();
    assert!(
        pre_restart_count > 0,
        "must have pending edits before restart; got {pre_restart_count}"
    );

    let pre_restart_max_seq = h
        .device(D0)
        .pending_edits()
        .iter()
        .map(|e| e.local_sequence)
        .max()
        .expect("must have max seq");

    // --- Phase 2: legacy sequence reset -----------------------------------------

    h.restart_legacy(D0);

    // --- Phase 3: post-restart edits — reuse low sequence numbers ---------------

    // Add two distinctive lines; they will get seq 1 and 2 (reset).
    h.append_line(D0, scheme, "line AFTER-RESTART-1");
    h.append_line(D0, scheme, "line AFTER-RESTART-2");

    // Verify duplicate sequences exist.
    {
        let all_seqs: Vec<u64> = h
            .device(D0)
            .pending_edits()
            .iter()
            .map(|e| e.local_sequence)
            .collect();
        let unique: std::collections::HashSet<_> = all_seqs.iter().collect();
        assert!(
            unique.len() < all_seqs.len(),
            "must have duplicate sequences after legacy restart+edits; got {:?}",
            all_seqs
        );
    }

    // --- Phase 4: sync — the first push batch covers the pre-restart edits ------
    //
    // `build_push_request` calls `pending_for_document(scheme_doc, 50)`, which takes
    // the FIRST 50 entries in the deque (the pre-restart ones).  The post-restart
    // entries (seq 1, 2) are positions 51+ and are NOT included in the first batch.
    //
    // After the batch is acked:
    //   mark_pushed(scheme_doc, pre_restart_max_seq)
    // clears ALL pending entries with seq <= pre_restart_max_seq.  The post-restart
    // entries have seq 1 and 2 which are <= pre_restart_max_seq, so they are DROPPED.
    //
    // DESIRED: the sync engine detects the dropped edits and re-pushes them (or
    // includes them in subsequent batches without dropping via a monotone cursor).
    //
    // TODAY: they are silently discarded.

    for attempt in 0..4 {
        let result = h.try_sync(D0);
        match result {
            Ok(()) if h.device(D0).is_fully_pushed() => break,
            Ok(()) => {}
            Err(e) => {
                eprintln!("[restart_with_pending] attempt {attempt} failed: {e:?}");
            }
        }
    }

    assert!(
        h.device(D0).is_fully_pushed(),
        "D0 pending queue must be empty after syncs; {} remain",
        h.device(D0).pending_count()
    );

    // D1 must see the post-restart lines.  TODAY this fails because the post-restart
    // lines were silently dropped by mark_pushed before they could be pushed.
    h.sync(D1);

    let d1_items = h.device(D1).scheme_item_texts(scheme);
    assert!(
        d1_items.contains(&"line AFTER-RESTART-1".to_string()),
        "D1 must see 'line AFTER-RESTART-1'; got {:?} \n\
         (today: dropped by mark_pushed due to duplicate seq <= pre_restart_max={pre_restart_max_seq})",
        d1_items,
    );
    assert!(
        d1_items.contains(&"line AFTER-RESTART-2".to_string()),
        "D1 must see 'line AFTER-RESTART-2'; got {:?}",
        d1_items,
    );

    h.assert_all_converged();

    let _ = pre_restart_max_seq; // used in the assertion message above
}

// ---------------------------------------------------------------------------
// Bug 1b — mark_pushed_clears_only_sent_edits  (unit-level)
// ---------------------------------------------------------------------------

/// White-box unit test at the `LocalSyncState` level: directly inject pending edits
/// that have duplicate `local_sequence` values (simulating what happens after a
/// legacy sequence reset), then acknowledge only the first batch.  The second
/// batch's edits must survive — they were never sent.
///
/// **Today's behavior**: `mark_pushed(doc, through=2)` clears `seq <= 2`, which
/// includes the *post-restart* seq=1 and seq=2 edits that were never sent.
#[test]
fn mark_pushed_clears_only_sent_edits() {
    use chrono::Utc;
    use knotq_model::{DocumentId, OperationId, ReplicaId, SyncDocumentKind, WorkspaceId};
    use knotq_sync::{LocalSyncState, PendingCrdtEdit};

    let doc = DocumentId::new();
    let workspace_id = WorkspaceId::new();
    let replica_id = ReplicaId::new();
    let kind = SyncDocumentKind::Scheme;

    // A non-empty but otherwise arbitrary valid Yrs update (empty doc snapshot).
    // We only need bytes that pass the VecDeque machinery; validation isn't called here.
    let dummy_update = vec![0u8, 0, 0]; // minimal placeholder — not decoded in mark_pushed

    let make_edit = |seq: u64| PendingCrdtEdit {
        operation_id: OperationId::new(),
        workspace_id,
        replica_id,
        local_sequence: seq,
        created_at: Utc::now(),
        document: doc,
        kind,
        update_v1: dummy_update.clone(),
    };

    let mut state = LocalSyncState {
        workspace_id: Some(workspace_id),
        replica_id: Some(replica_id),
        ..LocalSyncState::default()
    };

    // --- Pre-restart batch: seq 1, 2 (pending, not yet pushed) -----------------
    state.push_pending(make_edit(1));
    state.push_pending(make_edit(2));

    // --- Simulated restart: next_sequence reset to 1 (the bug) -----------------
    // New edits get seq 1, 2 again.
    state.push_pending(make_edit(1)); // post-restart edit, seq 1 — DUPLICATE
    state.push_pending(make_edit(2)); // post-restart edit, seq 2 — DUPLICATE

    // We now have 4 pending edits: [1, 2, 1, 2].
    assert_eq!(
        state.pending.len(),
        4,
        "should have 4 pending edits before ack"
    );

    // The server accepts the first push (the pre-restart batch, seq 1 and 2).
    // mark_pushed should clear only those two edits — not the post-restart ones.
    state.mark_pushed(doc, 2);

    // DESIRED: 2 post-restart edits (the second seq=1 and seq=2) survive.
    // TODAY:   mark_pushed removes all seq <= 2, so all 4 are dropped — this is the bug.
    assert_eq!(
        state.pending.len(),
        2,
        "mark_pushed must leave the 2 post-restart (never-sent) edits intact; \
         got {} remaining (today this is 0, demonstrating the bug)",
        state.pending.len(),
    );
}

// ---------------------------------------------------------------------------
// Bug 2 — schema_invalid_rejection_must_self_heal
// ---------------------------------------------------------------------------

/// Directly construct a wedged state that exactly mirrors the production scenario:
/// the client successfully pushes a first batch (server has partial state), but
/// `mark_pushed` silently drops later pending edits with duplicate sequence numbers
/// (the Bug 1 side-effect).  Subsequent edits then reference a causal base the server
/// never received.  When those edits are pushed, the server rejects them with
/// `crdt_schema_invalid`.  The engine must self-heal by re-pushing a full snapshot.
///
/// The wedge is constructed via the real `mark_pushed` API (not surgery) to ensure we
/// exercise exactly the code path that failed in production.
///
/// **Today's behavior**: the push is rejected, the error is returned to the caller,
/// and the next cycle retries the same bad deltas → permanent wedge.
///
/// **Desired behavior**: the engine detects the partial-server-state condition and
/// re-pushes a full snapshot to reseed the server, after which subsequent deltas apply
/// cleanly.
#[test]
fn schema_invalid_rejection_must_self_heal() {
    use chrono::Utc;
    use knotq_model::{OperationId, SyncDocumentKind};
    use knotq_sync::PendingCrdtEdit;

    // Use a Harness with one device to avoid any pull-from-server interference.
    let mut h = Harness::new(1);
    h.login_all();

    // D0 creates a scheme, syncs — server now has a base for both docs.
    let scheme = h.add_scheme(D0, "Healable", &["v1"]);
    h.sync(D0);

    // D0 makes two more edits offline (not pushed).  The pending queue now holds
    // incremental Yrs updates for "v2" and "v3".
    h.append_line(D0, scheme, "v2");
    h.append_line(D0, scheme, "v3");

    // -------------------------------------------------------------------------
    // Manufacture the Bug 1 side-effect directly at the LocalSyncState level:
    //
    // 1. Record the scheme_doc_id.
    // 2. Push a *fake* extra pending edit whose local_sequence equals the HIGHEST
    //    currently-pending sequence for that document — this duplicates a sequence
    //    number, exactly as the legacy restart would.
    // 3. Acknowledge the first push (the real pending edits) via mark_pushed.
    //    mark_pushed clears `seq <= through`, which removes the fake duplicate too,
    //    but ALSO leaves the server without the "v3" update (the fake replaced it
    //    in the queue and got cleared).
    // 4. Now push a new real edit ("v4") that was diffed against the post-v3 CRDT.
    //    The server has only v1 + the first push's merged state; the v4 delta is
    //    causally rooted after v3, which the server never received → schema_invalid.
    // -------------------------------------------------------------------------

    // Step 1: capture scheme doc id and highest pending seq.
    let (scheme_doc_id, highest_seq) = {
        let device = h.device(D0);
        let scheme_doc_id = device
            .workspace
            .scheme_sync
            .get(&scheme)
            .expect("scheme must have sync meta")
            .id;
        let highest_seq = device
            .pending_edits()
            .iter()
            .filter(|e| e.document == scheme_doc_id)
            .map(|e| e.local_sequence)
            .max()
            .expect("must have pending edits for scheme doc");
        (scheme_doc_id, highest_seq)
    };

    // Step 2: inject a fake pending edit with the same local_sequence as highest_seq.
    // This simulates the post-restart duplicate (the real edit for that sequence is
    // already in pending; we're adding a second one with the same number).
    {
        let device = h.device_mut_for_surgery(D0);
        let workspace_id = device.workspace.id;
        let replica_id = device.local_state_ref().replica_id.unwrap();
        // Use a tiny but syntactically valid Yrs empty-doc update as the payload.
        // The server never sees this (it gets dropped by mark_pushed first).
        let dummy: Vec<u8> = vec![0, 0]; // minimal placeholder
        device.local_state_mut().push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id,
            replica_id,
            local_sequence: highest_seq, // DUPLICATE — this is the bug
            created_at: Utc::now(),
            document: scheme_doc_id,
            kind: SyncDocumentKind::Scheme,
            update_v1: dummy,
        });
    }

    // Verify that we now have a duplicate sequence in pending.
    {
        let device = h.device(D0);
        let seqs: Vec<u64> = device
            .pending_edits()
            .iter()
            .filter(|e| e.document == scheme_doc_id)
            .map(|e| e.local_sequence)
            .collect();
        let unique: std::collections::HashSet<_> = seqs.iter().collect();
        assert!(
            unique.len() < seqs.len(),
            "must have duplicate sequences for scheme doc at this point; got {:?}",
            seqs
        );
    }

    // Step 3: simulate the server accepting the first push up through highest_seq.
    // This is what mark_pushed does when the server acks — it drops ALL pending
    // for this doc with seq <= through, including the duplicate fake edit AND the
    // real "v3" update (if it also has the same sequence).
    {
        let device = h.device_mut_for_surgery(D0);
        let dropped = device
            .local_state_mut()
            .mark_pushed(scheme_doc_id, highest_seq);
        assert!(
            dropped > 0,
            "mark_pushed must have dropped some edits; dropped={dropped}"
        );
    }

    // Step 4: D0 makes a NEW real edit ("v4") — this delta is causally rooted
    // after "v3", which the server never received (it was part of what mark_pushed
    // dropped).  When pushed, the server must reject it.
    h.append_line(D0, scheme, "v4");

    // Force a crdt_schema_invalid rejection deterministically so the recovery path
    // is always exercised, regardless of whether Yrs would have accepted the delta.
    // The engine detects the rejection, reseeds the affected document with a full
    // snapshot, and retries the push in the same call — so try_sync returns Ok(()).
    h.reject_next_push_with_schema_invalid();

    // The engine self-heals within this single sync call: it intercepts the forced
    // crdt_schema_invalid, drops the bad pending edits for the scheme document, and
    // re-queues a full snapshot, then retries.  The retry succeeds, so try_sync
    // must return Ok(()) and the pending queue must be drained.
    h.try_sync(D0)
        .expect("engine must self-heal from crdt_schema_invalid and return Ok");

    assert!(
        h.device(D0).is_fully_pushed(),
        "pending queue must be empty after self-heal sync; {} remain",
        h.device(D0).pending_count(),
    );
}

// ---------------------------------------------------------------------------
// Bug 1+2 — image_media_syncs_between_devices
// ---------------------------------------------------------------------------

/// Device A attaches a PNG image to a scheme item and syncs; device B pulls and
/// downloads it.  Both the CRDT metadata (ItemMedia reference in the item) and the
/// raw bytes must arrive on B.
///
/// Sub-assertion: when D0 has not yet synced (simulating a push backlog), D1 does
/// NOT get the new item or the bytes — demonstrating that media sync is gated behind
/// CRDT push completion (the "png not synced" user symptom in the wedge scenario).
#[test]
fn image_media_syncs_between_devices() {
    let mut h = Harness::new(2);
    h.login_all();

    // D0 creates a scheme, syncs to establish a shared base on the server.
    let scheme = h.add_scheme(D0, "Moodboard", &["caption"]);
    h.sync(D0);
    h.sync(D1); // D1 discovers the scheme

    // D0 attaches a synthetic PNG to item 0.
    let png_bytes: Vec<u8> = (0u8..64).collect(); // 64-byte synthetic PNG payload
    let (_asset_uuid, image_name) = h.attach_image_to_device(D0, scheme, 0, png_bytes.clone());

    // --- Sub-assertion: before D0 pushes, D1 sees nothing new -------------------
    // D1 pulls before D0 has synced the image edit; must NOT see it.
    h.sync(D1);
    {
        let has_media_before = h
            .device(D1)
            .workspace
            .schemes
            .get(&scheme)
            .and_then(|s| s.items.first())
            .map(|item| !item.media.is_empty())
            .unwrap_or(false);
        assert!(
            !has_media_before,
            "D1 must NOT see the image before D0 has pushed the CRDT edit",
        );
        let bytes_before = h.device(D1).media_assets.get(&image_name).cloned();
        assert!(
            bytes_before.is_none(),
            "D1 must NOT have the PNG bytes before D0 has uploaded them",
        );
    }

    // --- D0 syncs (CRDT push) and uploads media --------------------------------
    h.sync(D0);
    assert!(
        h.device(D0).is_fully_pushed(),
        "D0 pending queue must be empty after sync",
    );
    {
        let remote_latest: std::collections::HashMap<_, _> = h
            .device(D0)
            .local_state_ref()
            .document_cursors
            .values()
            .map(|c| (c.document, c.last_pulled_sequence))
            .collect();
        h.upload_media(D0, &remote_latest)
            .expect("D0 media upload must succeed");
    }

    // Server must now hold the media asset.
    assert_eq!(
        h.server_media_asset_count(),
        1,
        "server must hold 1 media asset after D0 upload",
    );

    // --- D1 pulls CRDT and downloads the bytes ---------------------------------
    h.sync(D1);
    h.download_media(D1);

    // DESIRED: D1 has the ItemMedia reference in item 0.
    {
        let has_media = h
            .device(D1)
            .workspace
            .schemes
            .get(&scheme)
            .and_then(|s| s.items.first())
            .map(|item| !item.media.is_empty())
            .unwrap_or(false);
        assert!(
            has_media,
            "D1 item 0 must have an ItemMedia::Image after pulling the CRDT edit",
        );
    }

    // DESIRED: D1 has the raw PNG bytes.
    {
        let d1_bytes = h.device(D1).media_assets.get(&image_name).cloned();
        assert_eq!(
            d1_bytes,
            Some(png_bytes.clone()),
            "D1 must have the exact PNG bytes after download from server",
        );
    }

    // --- Sub-assertion: media upload enforces 3 MiB limit ---------------------
    // A too-large asset must be rejected by the server.
    let oversized: Vec<u8> = vec![0xAB; knotq_sync::MAX_SYNC_MEDIA_BYTES + 1];
    let (_doc_id, over_name) = {
        let scheme2 = h.add_scheme(D0, "LargeImage", &["item"]);
        h.sync(D0);
        let (_, name) = h.attach_image_to_device(D0, scheme2, 0, oversized.clone());
        let scheme_doc_id = h.device(D0).workspace.scheme_sync.get(&scheme2).unwrap().id;
        (scheme_doc_id, name)
    };
    {
        // Attempt to upload the oversized asset — must fail.
        let remote_latest: std::collections::HashMap<_, _> = h
            .device(D0)
            .local_state_ref()
            .document_cursors
            .values()
            .map(|c| (c.document, c.last_pulled_sequence))
            .collect();
        let upload_result = h.upload_media(D0, &remote_latest);
        assert!(
            upload_result.is_err(),
            "uploading a {} byte asset (> {} byte limit) must fail; got {:?}",
            oversized.len(),
            knotq_sync::MAX_SYNC_MEDIA_BYTES,
            upload_result,
        );
    }
    let _ = over_name;
}

// ---------------------------------------------------------------------------
// Bug 3 — Daily Queue created by direct workspace mutation (2026-06-11 wedge)
// ---------------------------------------------------------------------------

/// Replays the exact on-disk state of the 2026-06-11 production wedge: the
/// desktop created today's Daily Queue scheme by direct workspace mutation, so
/// the scheme's CRDT document existed but was EMPTY (no `schema` root), and the
/// pending queue held a workspace-index delta plus the scheme's 2-byte empty
/// "snapshot". The server rejected the atomic batch as `crdt_schema_invalid`,
/// and the push self-heal reseeded the same empty snapshot forever.
///
/// The bootstrap must repair the schema-less document from the materialized
/// workspace, replace the bad pending edit with the healed full snapshot, and
/// converge both devices on the daily scheme's real content.
#[test]
fn wedged_empty_daily_snapshot_self_heals() {
    use knotq_model::SyncDocumentKind;

    let mut h = Harness::new(2);
    h.login_all();

    // An ordinary synced workspace, so the workspace document already has a
    // server base (as in production).
    h.add_scheme(D0, "Notes", &["seed"]);
    h.settle();

    let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 11).unwrap();
    let daily = h.set_daily_queue_without_crdt_content(D0, today, &["recovered task"]);
    // The workspace-index delta (the daily_queue map entry) rides in the same
    // push batch as the broken scheme snapshot, exactly as observed.
    h.record_workspace_change_pub(D0);
    // The empty doc's "snapshot" — a 2-byte Yjs update with no schema root — is
    // already queued, as captured from the wedged client's sync-state.json.
    let document = h.device(D0).scheme_document_id(daily);
    h.device_mut_for_surgery(D0)
        .push_raw_pending_edit(document, SyncDocumentKind::Scheme, vec![0, 0]);

    h.settle();
    h.assert_all_converged();
    for key in h.device_keys() {
        h.assert_scheme_items(key, daily, &["recovered task"]);
        assert_eq!(
            h.device(key).workspace.daily_queue_scheme_id(today),
            Some(daily),
            "{key:?}: daily queue entry missing"
        );
    }
    assert!(
        h.device(D0).is_fully_pushed(),
        "pending queue must drain after the heal"
    );
}
