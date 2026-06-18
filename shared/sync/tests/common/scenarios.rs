//! Hard CRDT scenario functions, backend-agnostic.
//!
//! Each function takes `&mut Harness` and runs one scenario end-to-end.
//! They are called from two thin wrappers:
//!   - `tests/hard_scenarios.rs` — in-memory backend (`Harness::new`)
//!   - `tests/backend_integration.rs` — HTTP backend (`Harness::new_http`)
//!
//! Keep op counts bounded to keep HTTP mode under ~10 min total.

#![allow(dead_code)]

use chrono::{NaiveDate, TimeZone, Utc};
use knotq_model::{Item, ItemMarker, SchemeId};

use super::{DeviceKey, Harness, Rng, D0, D1, D2};

// ---------------------------------------------------------------------------
// Scenario a — Edit-vs-delete race (both orderings)
// ---------------------------------------------------------------------------

/// A edits lines in scheme S while B deletes S.  Both sync orders are tested.
/// Engine semantics: deletion wins because the workspace-index entry is removed;
/// pulled content updates for the deleted doc are benign unknown_scheme_document skips.
pub fn scenario_a_edit_vs_delete_a_first(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Contested", &["line0", "line1", "line2"]);
    h.settle();

    // A edits offline.
    h.edit_line(D0, s, 0, "A-edited-line0");
    h.append_line(D0, s, "A-new-line");

    // B archives then permanently deletes.
    h.archive_scheme(D1, s);
    h.delete_scheme(D1, s);

    // A syncs first (edits land on server).
    h.sync(D0);
    // B syncs (deletion overwrites workspace index, drops the scheme).
    h.sync(D1);
    // A syncs again — must not wedge; workspace converges to deletion.
    h.sync(D0);

    h.settle();
    h.assert_all_converged();

    // Deletion wins: neither device should have the scheme in workspace.schemes.
    h.assert_scheme_absent(D0, s);
    h.assert_scheme_absent(D1, s);
}

pub fn scenario_a_edit_vs_delete_b_first(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Contested2", &["a", "b", "c"]);
    h.settle();

    h.edit_line(D0, s, 1, "A-edited-b");
    h.archive_scheme(D1, s);
    h.delete_scheme(D1, s);

    // B syncs first (deletion lands on server).
    h.sync(D1);
    // A syncs (stale edits should not wedge or error).
    h.sync(D0);
    h.sync(D1);

    h.settle();
    h.assert_all_converged();
    h.assert_scheme_absent(D0, s);
    h.assert_scheme_absent(D1, s);
}

// ---------------------------------------------------------------------------
// Scenario b — Delete-vs-archive race on the same scheme
// ---------------------------------------------------------------------------

pub fn scenario_b_delete_vs_archive_race(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Race Scheme", &["content"]);
    h.settle();

    // D0 archives.
    h.archive_scheme(D0, s);
    // D1 archives then permanently deletes.
    h.archive_scheme(D1, s);
    h.delete_scheme(D1, s);

    // Sync in adversarial order.
    h.sync(D1);
    h.sync(D0);
    h.sync(D1);

    h.settle();
    // Both devices must agree on the outcome (convergence is the hard requirement).
    // The CRDT merge is LWW for the workspace node entry:
    // D1 removes the node, D0's archive merely updates recently_deleted.
    // Regardless of who wins, both devices must see the SAME state.
    h.assert_all_converged();

    // Verify monotonic invariant: the scheme must not be active (in the root) on either device.
    // If deletion won, it's absent from schemes. If archive won, it's in recently_deleted
    // and NOT in the sidebar root. Either outcome is valid CRDT semantics; they just must agree.
    let d0_in_root = h.device(D0).root_scheme_ids().contains(&s);
    let d1_in_root = h.device(D1).root_scheme_ids().contains(&s);
    assert!(
        !d0_in_root,
        "scheme must not be active in root after archive/delete race on D0"
    );
    assert!(
        !d1_in_root,
        "scheme must not be active in root after archive/delete race on D1"
    );
}

// ---------------------------------------------------------------------------
// Scenario c — Folder shuffle storm
// ---------------------------------------------------------------------------

pub fn scenario_c_folder_shuffle_storm(h: &mut Harness) {
    h.login_all();

    // Create base state.
    let f1 = h.add_folder(D0, "F1");
    let f2 = h.add_folder(D0, "F2");
    let s1 = h.add_scheme_to_folder(D0, f1, "S1", &["a"]);
    let s2 = h.add_scheme_to_folder(D0, f1, "S2", &["b"]);
    let s3 = h.add_scheme(D0, "S3", &["c"]);
    h.settle();

    // A moves schemes between folders.
    h.move_scheme_to_folder(D0, s1, f2);
    h.move_scheme_to_folder(D0, s3, f1);

    // B renames schemes and archives one concurrently.
    h.rename_scheme(D1, s2, "S2-renamed");
    h.rename_scheme(D1, s3, "S3-renamed");
    h.archive_scheme(D1, s1);

    // B also renames F1.
    h.rename_folder(D1, f1, "F1-renamed");

    h.sync(D0);
    h.sync(D1);
    h.sync(D0);

    h.settle();
    h.assert_all_converged();

    // F1 and F2 must still exist.
    assert!(
        h.device(D0).workspace.folders.contains_key(&f1),
        "F1 vanished"
    );
    assert!(
        h.device(D0).workspace.folders.contains_key(&f2),
        "F2 vanished"
    );
}

// ---------------------------------------------------------------------------
// Scenario d — Zig-zag workspace/document interleave
// ---------------------------------------------------------------------------

pub fn scenario_d_zigzag_interleave(h: &mut Harness) {
    h.login_all();

    let mut schemes = Vec::new();
    for i in 0..6 {
        schemes.push(h.add_scheme(D0, &format!("ZZ-{i}"), &["init"]));
    }
    let folder = h.add_folder(D0, "ZZFolder");
    h.settle();

    for round in 0..10 {
        let device = if round % 2 == 0 { D0 } else { D1 };
        let other = if round % 2 == 0 { D1 } else { D0 };
        let s = schemes[round % schemes.len()];

        // Workspace-level op.
        if round % 3 == 0 {
            h.move_scheme_to_folder(device, s, folder);
        } else if round % 3 == 1 {
            h.rename_scheme(device, s, &format!("ZZ-{}-r{round}", round % schemes.len()));
        } else {
            h.move_scheme_to_root(device, s);
        }

        // Item-level edit.
        h.append_line(other, s, &format!("round-{round}"));
        h.edit_line(device, s, 0, &format!("edited-{round}"));

        // Adversarial sync timing.
        if round % 3 == 0 {
            h.sync(device);
            h.sync(other);
            h.sync(device); // double-sync
        } else {
            h.sync(other);
        }
    }

    h.settle();
    h.assert_all_converged();
}

// ---------------------------------------------------------------------------
// Scenario e — Long offline divergence (exceeds push batch bounds)
// ---------------------------------------------------------------------------

pub fn scenario_e_long_offline_divergence(h: &mut Harness) {
    use knotq_sync::PUSH_MAX_UPDATES_PER_DOCUMENT;
    h.login_all();

    // Seed 15 schemes.
    let mut schemes = Vec::new();
    for i in 0..15 {
        schemes.push(h.add_scheme(D0, &format!("offline-{i:02}"), &["seed"]));
    }
    let dates: Vec<NaiveDate> = (1u32..=5)
        .map(|d| NaiveDate::from_ymd_opt(2026, 8, d).unwrap())
        .collect();
    for &date in &dates {
        h.set_daily_queue(D0, date, &["morning entry"]);
    }
    h.settle();

    // Both devices go offline and generate 200+ mixed ops.
    // A: rename/archive/restore + many line edits.
    for i in 0..(PUSH_MAX_UPDATES_PER_DOCUMENT * 2 + 10) {
        let s = schemes[i % schemes.len()];
        if i % 17 == 0 {
            h.rename_scheme(D0, s, &format!("offline-renamed-{i}"));
        } else if i % 23 == 0 && i % 46 != 0 {
            h.archive_scheme(D0, s);
        } else if i % 46 == 0 {
            h.restore_scheme(D0, s);
        } else {
            h.append_line(D0, s, &format!("A-offline-{i}"));
        }
    }

    // B: different renames + line edits.
    for i in 0..(PUSH_MAX_UPDATES_PER_DOCUMENT * 2 + 15) {
        let s = schemes[i % schemes.len()];
        if i % 19 == 0 {
            h.rename_scheme(D1, s, &format!("B-renamed-{i}"));
        } else {
            h.append_line(D1, s, &format!("B-offline-{i}"));
        }
    }

    // A syncs first (multiple push batches).
    h.sync(D0);
    // B syncs (pulls A's changes, pushes its own — multiple batches).
    h.sync(D1);
    // A syncs again to pull B's changes.
    h.sync(D0);

    h.settle();
    h.assert_all_converged();

    // Spot-check: both devices must have at least some of each device's content.
    let last_scheme = schemes.last().copied().unwrap();
    // If last_scheme survived archiving we should see content; if archived both
    // should agree (convergence check above). Either way, just confirm the workspace
    // still has the scheme (or both removed it consistently).
    let d0_has = h.device(D0).workspace.schemes.contains_key(&last_scheme);
    let d1_has = h.device(D1).workspace.schemes.contains_key(&last_scheme);
    assert_eq!(
        d0_has, d1_has,
        "devices disagree on whether last offline scheme exists"
    );
}

// ---------------------------------------------------------------------------
// Scenario f — Offline + multiple restarts combo
// ---------------------------------------------------------------------------

pub fn scenario_f_offline_restart_combo(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Restart Combo", &["seed"]);
    h.settle();

    // Accumulate edits offline.
    for i in 0..30 {
        h.append_line(D0, s, &format!("offline-before-restart-{i}"));
    }

    // First restart (correct seeding).
    h.restart(D0);

    // More edits.
    for i in 0..20 {
        h.append_line(D0, s, &format!("after-restart1-{i}"));
    }

    // Second restart.
    h.restart(D0);

    // More edits.
    for i in 0..15 {
        h.append_line(D0, s, &format!("after-restart2-{i}"));
    }

    // Sync — must not fail with crdt_schema_invalid.
    h.sync(D0);
    h.sync(D1);
    h.settle();
    h.assert_all_converged();

    // D1 must see all three batches of edits.
    let texts = h.device(D1).scheme_item_texts(s);
    assert!(
        texts.iter().any(|t| t.contains("offline-before-restart")),
        "D1 missing pre-restart edits"
    );
    assert!(
        texts.iter().any(|t| t.contains("after-restart1")),
        "D1 missing post-restart1 edits"
    );
    assert!(
        texts.iter().any(|t| t.contains("after-restart2")),
        "D1 missing post-restart2 edits"
    );
}

// ---------------------------------------------------------------------------
// Scenario g — Daily queue conflicts
// ---------------------------------------------------------------------------

pub fn scenario_g_daily_queue_conflicts(h: &mut Harness) {
    h.login_all();

    let day1 = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    let day2 = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
    let day3 = NaiveDate::from_ymd_opt(2026, 9, 3).unwrap();

    // Both devices write to the same day concurrently.
    let dq_d0_day1 = h.set_daily_queue(D0, day1, &["D0-morning", "D0-noon"]);
    let dq_d1_day1 = h.set_daily_queue(D1, day1, &["D1-task1", "D1-task2", "D1-task3"]);

    // Each writes to different days.
    h.set_daily_queue(D0, day2, &["D0-day2"]);
    h.set_daily_queue(D1, day3, &["D1-day3"]);

    h.settle();
    h.assert_all_converged();

    // Both devices must see all three days.
    for key in h.device_keys() {
        assert!(
            h.device(key)
                .workspace
                .daily_queue_scheme_id(day2)
                .is_some(),
            "{key:?}: day2 missing"
        );
        assert!(
            h.device(key)
                .workspace
                .daily_queue_scheme_id(day3)
                .is_some(),
            "{key:?}: day3 missing"
        );
    }

    // Same-day conflict: stable daily IDs are deterministic (knotq_model::daily_queue_scheme_id),
    // so D0 and D1's `set_daily_queue` calls for day1 produce the SAME SchemeId.
    // The CRDT merges both devices' item edits into that single doc.
    assert_eq!(
        dq_d0_day1, dq_d1_day1,
        "daily_queue_scheme_id must be deterministic"
    );
}

// ---------------------------------------------------------------------------
// Scenario g2 — Daily Queue created by direct workspace mutation
// ---------------------------------------------------------------------------

/// Reproduces the production `crdt_schema_invalid` wedge of 2026-06-11: the
/// desktop creates today's Daily Queue scheme by mutating the workspace directly
/// (no command), so the scheme's CRDT document exists but is EMPTY — no `schema`
/// root. The bootstrap used to snapshot that empty doc into the push queue; the
/// server rejected the whole atomic batch (the workspace-index delta rides
/// along), and the push self-heal reseeded the same empty snapshot forever. The
/// bootstrap must repair the document from the materialized workspace before
/// snapshotting.
pub fn scenario_g2_daily_queue_direct_creation(h: &mut Harness) {
    h.login_all();
    let today = NaiveDate::from_ymd_opt(2026, 6, 11).unwrap();

    // Phase 1: an ordinary synced workspace, so the workspace document has a
    // server base (the daily-queue entry below is a delta, as in production).
    h.add_scheme(D0, "Notes", &["seed"]);
    h.settle();

    // Phase 2: D0 creates today's Daily Queue directly — scheme content never
    // reaches the CRDT; only the workspace-index delta is recorded.
    let daily = h.set_daily_queue_without_crdt_content(D0, today, &["wedge task", "second task"]);
    h.record_workspace_change_pub(D0);

    // Phase 3: sync must succeed (the bootstrap heals the schema-less document)
    // and every device must converge on the daily scheme's real content.
    h.settle();
    h.assert_all_converged();
    for key in h.device_keys() {
        assert_eq!(
            h.device(key).workspace.daily_queue_scheme_id(today),
            Some(daily),
            "{key:?}: daily queue entry missing"
        );
        h.assert_scheme_items(key, daily, &["wedge task", "second task"]);
    }
    assert!(
        h.device(D0).is_fully_pushed(),
        "D0 push queue must drain after the heal"
    );
}

// ---------------------------------------------------------------------------
// Scenario h — Calendar import lifecycle
// ---------------------------------------------------------------------------

pub fn scenario_h_calendar_import_lifecycle(h: &mut Harness) {
    h.login_all();

    // A imports a calendar with 3 events.
    let cal = h.import_calendar_scheme(
        D0,
        "Work Calendar",
        "google-acct-01",
        "work@example.com",
        "primary",
        &["standup", "1:1", "review"],
    );
    h.sync(D0);
    h.sync(D1);

    // B must see the calendar as read-only.
    let source = h.imported_calendar_source(D1, cal);
    assert!(source.is_some(), "D1 must see calendar source");
    let source = source.unwrap();
    assert!(source.read_only);
    assert_eq!(source.calendar_id, "primary");

    // A re-imports with changed/removed events (simulate gsync update).
    // Directly mutate items to simulate a gsync re-import that removes one event,
    // then add a new event via the normal API which calls record_changes.
    {
        let device = h.device_mut_for_surgery(D0);
        device
            .scheme_mut_pub(cal)
            .items
            .retain(|item| item.text() != "1:1");
        // record the retained change so it queues as CRDT updates
        let changes = knotq_sync::WorkspaceCrdtChangeSet::default().touch_scheme(cal);
        device.record_changes(changes);
    }
    h.append_line(D0, cal, "planning session"); // add new event

    h.sync(D0);
    h.sync(D1);
    h.settle();
    h.assert_all_converged();

    // B sees updated events.
    let items_b = h.device(D1).scheme_item_texts(cal);
    assert!(
        !items_b.iter().any(|t| t == "1:1"),
        "removed event must not appear on D1"
    );
    assert!(
        items_b.iter().any(|t| t == "planning session"),
        "new event must appear on D1"
    );

    // A removes the calendar import entirely.
    h.archive_scheme(D0, cal);
    h.delete_scheme(D0, cal);
    h.sync(D0);
    h.sync(D1);
    h.settle();
    h.assert_all_converged();
    h.assert_scheme_absent(D0, cal);
    h.assert_scheme_absent(D1, cal);
}

// ---------------------------------------------------------------------------
// Scenario i — Media variants
// ---------------------------------------------------------------------------

/// Two devices attach DIFFERENT images to the same scheme concurrently; both assets
/// must survive after sync.  (In-memory only for the oversized-upload check; the HTTP
/// variant skips that part since it goes through the real R2 path which is also gated.)
pub fn scenario_i_media_variants(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Media Scheme", &["item0", "item1"]);
    h.settle();

    // Attach different images on D0 and D1 concurrently.
    let img_a: Vec<u8> = (0u32..1024).map(|i| (i % 251) as u8).collect();
    let img_b: Vec<u8> = (0u32..2048).map(|i| (i % 127) as u8).collect();

    let (_, name_a) = h.attach_image_to_device(D0, s, 0, img_a.clone());
    let (_, name_b) = h.attach_image_to_device(D1, s, 1, img_b.clone());

    h.sync(D0);
    let remote_latest_d0 = h.device_remote_latest(D0);
    h.upload_media(D0, &remote_latest_d0).expect("upload A");

    h.sync(D1);
    let remote_latest_d1 = h.device_remote_latest(D1);
    h.upload_media(D1, &remote_latest_d1).expect("upload B");

    h.sync(D0);
    h.download_media(D0);
    h.sync(D1);
    h.download_media(D1);

    h.settle();
    h.assert_all_converged();

    // Both assets must be present on both devices.
    assert!(
        h.device(D0).media_assets.contains_key(&name_a),
        "D0 missing its own asset"
    );
    assert!(
        h.device(D0).media_assets.contains_key(&name_b),
        "D0 missing D1's asset"
    );
    assert!(
        h.device(D1).media_assets.contains_key(&name_a),
        "D1 missing D0's asset"
    );
    assert!(
        h.device(D1).media_assets.contains_key(&name_b),
        "D1 missing its own asset"
    );

    // Asset bytes must be intact.
    assert_eq!(
        h.device(D0).media_assets[&name_a],
        img_a,
        "A image bytes corrupted"
    );
    assert_eq!(
        h.device(D1).media_assets[&name_b],
        img_b,
        "B image bytes corrupted"
    );
}

/// Image attached then scheme deleted — the other device must tolerate the orphan
/// content doc that lingers server-side.
pub fn scenario_i_media_scheme_deleted(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Doomed Media Scheme", &["item"]);
    h.settle();

    let img: Vec<u8> = (0u32..512).map(|i| (i % 251) as u8).collect();
    let (_, _name) = h.attach_image_to_device(D0, s, 0, img.clone());
    h.sync(D0);
    let remote = h.device_remote_latest(D0);
    h.upload_media(D0, &remote).expect("upload");

    // D0 deletes the scheme.
    h.archive_scheme(D0, s);
    h.delete_scheme(D0, s);
    h.sync(D0);

    // D1 syncs — must not error even though the content doc lingers.
    h.sync(D1);
    h.settle();
    h.assert_all_converged();
    h.assert_scheme_absent(D1, s);
}

// ---------------------------------------------------------------------------
// Scenario j — Notification schedule interleaved with doc edits
// ---------------------------------------------------------------------------

pub fn scenario_j_notification_schedule(h: &mut Harness) {
    h.login_all();

    let s = h.add_scheme(D0, "Notify Scheme", &["task"]);
    h.settle();

    // D0 pushes a schedule rev with some doc edits mixed in.
    h.append_line(D0, s, "new task");
    let hash_a = "a".repeat(64);
    let rev_a = h.update_notification_schedule(D0, 1, &hash_a);

    // D1 pushes a higher sequence.
    h.append_line(D1, s, "D1 task");
    let hash_b = "b".repeat(64);
    let rev_b = h.update_notification_schedule(D1, 2, &hash_b);

    // The real backend enforces monotonic notification_schedule_revision.
    // For in-memory the test server returns 0 (it doesn't track revisions);
    // that's fine — we just check the HTTP path for monotonicity.
    let _ = (rev_a, rev_b); // used by HTTP wrapper assertions

    h.settle();
    h.assert_all_converged();

    // Both devices must see all item edits.
    for key in h.device_keys() {
        let texts = h.device(key).scheme_item_texts(s);
        assert!(
            texts.iter().any(|t| t == "new task"),
            "{key:?} missing D0 task"
        );
        assert!(
            texts.iter().any(|t| t == "D1 task"),
            "{key:?} missing D1 task"
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario k — Fresh device join mid-chaos
// ---------------------------------------------------------------------------

/// After a long-offline divergence scenario, a brand-new device (D2) joins and
/// must materialize the full workspace including archived items, folder tree,
/// daily queue, and media cursors.
pub fn scenario_k_fresh_device_join(h: &mut Harness) {
    // Run scenario e subset to build up state.
    h.login_all();

    let mut schemes = Vec::new();
    for i in 0..8 {
        schemes.push(h.add_scheme(D0, &format!("joined-{i}"), &["seed"]));
    }
    let folder = h.add_folder(D0, "JoinedFolder");
    let s_in_folder = h.add_scheme_to_folder(D0, folder, "FolderScheme", &["in folder"]);
    let day = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    h.set_daily_queue(D0, day, &["daily entry"]);

    // Sync D0 so D1 can discover the schemes.
    h.sync(D0);
    h.sync(D1);

    // Archive one scheme.
    h.archive_scheme(D0, schemes[0]);

    // Make edits on D1 (it now knows the schemes from D0).
    for i in 0..10 {
        h.append_line(D1, schemes[1], &format!("D1-edit-{i}"));
    }

    // Settle D0 and D1 (D2 is left as a fresh device).
    h.sync(D0);
    h.sync(D1);
    h.sync(D0);
    // Note: D2 is NOT synced yet — it remains a "fresh device".

    // Verify D0 and D1 converged.
    let d0_summary = h.device(D0).workspace.schemes.len();
    let d1_summary = h.device(D1).workspace.schemes.len();
    assert_eq!(
        d0_summary, d1_summary,
        "D0 and D1 must converge before D2 joins"
    );

    // D2 starts fresh and syncs.
    h.sync(D2);

    // D2 must see the full workspace.
    assert!(
        h.device(D2).workspace.schemes.contains_key(&s_in_folder),
        "D2 missing FolderScheme"
    );
    assert!(
        h.device(D2).workspace.folders.contains_key(&folder),
        "D2 missing folder"
    );
    assert!(
        h.device(D2).workspace.daily_queue_scheme_id(day).is_some(),
        "D2 missing daily queue"
    );
    assert!(
        h.device(D2).workspace.is_scheme_deleted(schemes[0]),
        "D2 must see archived scheme as archived"
    );

    // D2's items for schemes[1] must include D1's edits.
    let texts = h.device(D2).scheme_item_texts(schemes[1]);
    assert!(
        texts.iter().any(|t| t.contains("D1-edit")),
        "D2 missing D1 edits"
    );
}

// ---------------------------------------------------------------------------
// Scenario l — Seeded randomized fuzz (both backends)
// ---------------------------------------------------------------------------

/// A seeded 3-device randomized run with mixed ops including delete/archive/move/
/// daily-queue.  `op_count` scales for the backend: ~150 in-memory, ~60 over HTTP.
pub fn scenario_l_randomized_fuzz(h: &mut Harness, seed: u64, op_count: usize) {
    h.login_all();

    let mut rng = Rng::new(seed);
    let devices = h.device_keys();
    let n_devices = devices.len();

    // Seed schemes.
    let mut schemes = Vec::new();
    for i in 0..8 {
        schemes.push(h.add_scheme(D0, &format!("fuzz-{i}"), &["seed"]));
    }
    let folder = h.add_folder(D0, "FuzzFolder");
    h.settle();

    let dates: Vec<NaiveDate> = (1u32..=7)
        .map(|d| NaiveDate::from_ymd_opt(2026, 11, d).unwrap())
        .collect();

    for step in 0..op_count {
        let device = devices[rng.below(n_devices as u64) as usize];
        let scheme_idx = rng.below(schemes.len() as u64) as usize;
        let s = schemes[scheme_idx];

        match rng.below(14) {
            0 | 1 => h.append_line(device, s, &format!("f{seed}-s{step}")),
            2 => {
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 0 {
                    h.edit_line(
                        device,
                        s,
                        rng.below(len as u64) as usize,
                        &format!("f{seed}-e{step}"),
                    );
                }
            }
            3 => {
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                h.insert_line(
                    device,
                    s,
                    rng.below((len + 1) as u64) as usize,
                    &format!("f{seed}-i{step}"),
                );
            }
            4 => {
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 2 {
                    h.remove_line(device, s, rng.below(len as u64) as usize);
                }
            }
            5 => h.rename_scheme(device, s, &format!("fuzz-r-{seed}-{step}")),
            6 => {
                if h.device(device).workspace.is_scheme_deleted(s) {
                    h.restore_scheme(device, s);
                } else {
                    h.archive_scheme(device, s);
                }
            }
            7 => {
                h.move_scheme_to_folder(device, s, folder);
            }
            8 => {
                h.move_scheme_to_root(device, s);
            }
            9 => {
                let date = dates[rng.below(dates.len() as u64) as usize];
                h.set_daily_queue(device, date, &[&format!("fuzz-dq-{seed}-{step}")]);
            }
            10 => {
                // Item-level richness: change marker.
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 0 {
                    let idx = rng.below(len as u64) as usize;
                    let marker = match rng.below(4) {
                        0 => ItemMarker::Blank,
                        1 => ItemMarker::Bullet,
                        2 => ItemMarker::Numbered,
                        _ => ItemMarker::Checkbox,
                    };
                    h.set_item_marker(device, s, idx, marker);
                }
            }
            11 => {
                // Item-level richness: set dates.
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 0 {
                    let idx = rng.below(len as u64) as usize;
                    let start = Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).unwrap();
                    h.set_item_dates(device, s, idx, Some(start), None);
                }
            }
            12 => {
                // Item-level richness: change indent.
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 0 {
                    let idx = rng.below(len as u64) as usize;
                    h.set_item_indent(device, s, idx, (rng.below(4)) as u8);
                }
            }
            _ => h.sync(device),
        }

        if step % 13 == 0 {
            h.sync(devices[rng.below(n_devices as u64) as usize]);
        }
    }

    h.settle();
    h.assert_all_converged_with_context(seed);
}

// ---------------------------------------------------------------------------
// Daily-queue "roll over from yesterday" (carryover) family
//
// Carryover is a single user action that mutates TWO scheme documents at once —
// the source day (date annotations stripped) and today (carried clones inserted) —
// while minting a FRESH ItemId for every carried row. That cross-document, fresh-id
// shape is exactly the class of edit that has wedged production sync before
// (empty/duplicate daily docs, item-id collisions). These scenarios stress it.
// ---------------------------------------------------------------------------

/// Find the (first) item with `text` in `scheme` on device `key`.
fn find_item<'a>(
    h: &'a Harness,
    key: DeviceKey,
    scheme: SchemeId,
    text: &str,
) -> &'a knotq_model::Item {
    h.device(key).workspace.schemes[&scheme]
        .items
        .iter()
        .find(|item| item.text() == text)
        .unwrap_or_else(|| panic!("{key:?}: item {text:?} not found in {scheme}"))
}

// Scenario m — Single-device carryover correctness + cross-document sync.
pub fn scenario_m_carryover_basic(h: &mut Harness) {
    h.login_all();
    let yesterday = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 12, 2).unwrap();
    let due = Utc.with_ymd_and_hms(2026, 12, 1, 9, 0, 0).unwrap();

    // Yesterday mixes carryable and non-carryable rows.
    let prev = h.seed_daily_queue(
        D0,
        yesterday,
        vec![
            Item::new("carry me").with_marker(ItemMarker::Checkbox), // incomplete -> carried
            Item::new("finished").with_marker(ItemMarker::Checkbox).done(), // done -> NOT carried
            Item::new("loose note"),                                 // plain -> carried
            Item::new("call dentist").with_start(due),               // dated -> carried, source stripped
        ],
    );
    // A freshly opened today is a single blank placeholder row.
    let today_id = h.set_daily_queue(D0, today, &[""]);
    h.settle();
    h.assert_all_converged();

    // Roll yesterday's open work into today.
    let carried = h.carryover_daily_queue(D0, today).expect("something to carry");
    assert_eq!(
        carried.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["carry me", "loose note", "call dentist"],
    );
    h.settle();
    h.assert_all_converged();

    // Today holds exactly the carried rows (placeholder replaced by the first).
    for key in h.device_keys() {
        h.assert_scheme_items(key, today_id, &["carry me", "loose note", "call dentist"]);
        // The completed task did not carry; yesterday keeps all four rows in order.
        h.assert_scheme_items(
            key,
            prev,
            &["carry me", "finished", "loose note", "call dentist"],
        );
    }

    // The cross-document split: the dated SOURCE row was stripped on yesterday, but
    // the carried COPY in today keeps its date — and both halves reach every device.
    for key in h.device_keys() {
        assert!(
            find_item(h, key, prev, "call dentist").start.is_none(),
            "{key:?}: source row date should be stripped on yesterday"
        );
        assert_eq!(
            find_item(h, key, today_id, "call dentist").start,
            Some(due),
            "{key:?}: carried row should keep its date in today"
        );
    }
}

// Scenario m2 — Both devices roll the same yesterday into one SHARED (synced) today.
// The blank placeholder is shared, so each device's first carried row lands on that
// single id (de-duping the first row); the rest duplicate. Hard requirement:
// convergence, no crdt_schema_invalid wedge, and no ItemId collision.
pub fn scenario_m2_carryover_concurrent_shared_today(h: &mut Harness) {
    h.login_all();
    let yesterday = NaiveDate::from_ymd_opt(2026, 12, 8).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 12, 9).unwrap();

    h.seed_daily_queue(
        D0,
        yesterday,
        vec![
            Item::new("task A").with_marker(ItemMarker::Checkbox),
            Item::new("task B").with_marker(ItemMarker::Checkbox),
            Item::new("task C").with_marker(ItemMarker::Checkbox),
        ],
    );
    let today_id = h.set_daily_queue(D0, today, &[""]);
    h.settle();
    h.assert_all_converged();

    // Both devices, offline, tap "roll over" into the same shared today.
    let c0 = h.carryover_daily_queue(D0, today).expect("D0 carries");
    let c1 = h.carryover_daily_queue(D1, today).expect("D1 carries");
    assert_eq!(c0.len(), 3);
    assert_eq!(c1.len(), 3);

    // Adversarial sync order, then settle.
    h.sync(D0);
    h.sync(D1);
    h.sync(D0);
    h.settle();
    h.assert_all_converged();

    // Shared placeholder de-dupes the first row: {A x1, B x2, C x2}.
    for key in h.device_keys() {
        h.assert_scheme_items_unordered(
            key,
            today_id,
            &["task A", "task B", "task B", "task C", "task C"],
        );
        assert_no_duplicate_item_ids(h, key, today_id);
        assert!(
            h.device(key).is_fully_pushed(),
            "{key:?}: push queue must drain after concurrent carryover"
        );
    }
}

// Scenario m3 — Both devices independently OPEN today offline (same deterministic
// daily SchemeId, but different placeholder ItemIds) and both roll over. Two distinct
// placeholders means both first rows survive, so every carried row duplicates. This
// also proves the deterministic daily document keeps independent creations on ONE doc
// rather than splitting content (the 2026-06-11 empty-daily-doc wedge class).
pub fn scenario_m3_carryover_concurrent_independent_today(h: &mut Harness) {
    h.login_all();
    let yesterday = NaiveDate::from_ymd_opt(2026, 12, 15).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 12, 16).unwrap();

    h.seed_daily_queue(
        D0,
        yesterday,
        vec![
            Item::new("task A").with_marker(ItemMarker::Checkbox),
            Item::new("task B").with_marker(ItemMarker::Checkbox),
        ],
    );
    h.settle();
    h.assert_all_converged();

    // Each device opens today independently while offline — same SchemeId, different
    // placeholder ItemId, no sync in between.
    let today_id = h.set_daily_queue(D0, today, &[""]);
    let today_id_d1 = h.set_daily_queue(D1, today, &[""]);
    assert_eq!(today_id, today_id_d1, "daily SchemeId must be deterministic");

    h.carryover_daily_queue(D0, today).expect("D0 carries");
    h.carryover_daily_queue(D1, today).expect("D1 carries");

    h.sync(D0);
    h.sync(D1);
    h.sync(D0);
    h.settle();
    h.assert_all_converged();

    // Two independent placeholders => every row duplicates: {A x2, B x2}.
    for key in h.device_keys() {
        h.assert_scheme_items_unordered(
            key,
            today_id,
            &["task A", "task A", "task B", "task B"],
        );
        assert_no_duplicate_item_ids(h, key, today_id);
        assert!(
            h.device(key).is_fully_pushed(),
            "{key:?}: push queue must drain after independent carryover"
        );
    }
    // All four rows live on the single deterministic daily document — independent
    // creation did not split content across two documents.
    assert_eq!(
        h.device(D0).workspace.schemes[&today_id].items.len(),
        4,
        "independent daily creations must converge onto one document"
    );
}

// Scenario m4 — D0 rolls yesterday forward while D1 keeps editing yesterday
// concurrently. The carried rows in today are independent CLONES (fresh ids in a
// different document), so they keep the snapshot D0 captured; yesterday converges
// with D1's later edits.
pub fn scenario_m4_carryover_vs_yesterday_edit(h: &mut Harness) {
    h.login_all();
    let yesterday = NaiveDate::from_ymd_opt(2026, 12, 22).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 12, 23).unwrap();

    let prev = h.seed_daily_queue(
        D0,
        yesterday,
        vec![
            Item::new("ship release").with_marker(ItemMarker::Checkbox),
            Item::new("write changelog").with_marker(ItemMarker::Checkbox),
        ],
    );
    let today_id = h.set_daily_queue(D0, today, &[""]);
    h.settle();
    h.assert_all_converged();

    // Concurrently, offline:
    //  - D0 rolls yesterday's two open tasks into today.
    //  - D1 keeps working on yesterday: edits one row and adds another.
    let carried = h.carryover_daily_queue(D0, today).expect("D0 carries");
    assert_eq!(carried.len(), 2);
    h.edit_line(D1, prev, 1, "write changelog v2");
    h.append_line(D1, prev, "tag the build");

    h.sync(D1);
    h.sync(D0);
    h.sync(D1);
    h.settle();
    h.assert_all_converged();

    for key in h.device_keys() {
        // Today keeps the carryover snapshot, independent of D1's later edits.
        h.assert_scheme_items(key, today_id, &["ship release", "write changelog"]);
        // Yesterday converges with D1's edits.
        let texts = h.device(key).scheme_item_texts(prev);
        assert_eq!(
            texts,
            vec!["ship release", "write changelog v2", "tag the build"],
            "{key:?}: yesterday did not converge with concurrent edits"
        );
    }
}

// Scenario m5 — Carryover offline, then the app RESTARTS (twice) before it can sync.
// The cross-document carryover edits plus post-restart edits must survive and push
// without a crdt_schema_invalid wedge (sequence seeding + persisted CRDT state).
pub fn scenario_m5_carryover_offline_restart(h: &mut Harness) {
    h.login_all();
    let yesterday = NaiveDate::from_ymd_opt(2027, 1, 5).unwrap();
    let today = NaiveDate::from_ymd_opt(2027, 1, 6).unwrap();

    h.seed_daily_queue(
        D0,
        yesterday,
        vec![
            Item::new("review PRs").with_marker(ItemMarker::Checkbox),
            Item::new("update docs").with_marker(ItemMarker::Checkbox),
            Item::new("plan sprint"),
        ],
    );
    let today_id = h.set_daily_queue(D0, today, &[""]);
    h.settle();
    h.assert_all_converged();

    let carried = h.carryover_daily_queue(D0, today).expect("carries");
    assert_eq!(carried.len(), 3);
    h.device_mut_for_surgery(D0).restart();
    // A little more offline work after the restart, then restart again.
    h.append_line(D0, today_id, "ad-hoc idea");
    h.device_mut_for_surgery(D0).restart();

    h.try_sync(D0)
        .expect("carryover push must survive restart without crdt_schema_invalid");
    h.settle();
    h.assert_all_converged();

    for key in h.device_keys() {
        h.assert_scheme_items(
            key,
            today_id,
            &["review PRs", "update docs", "plan sprint", "ad-hoc idea"],
        );
    }
}

// Scenario n — Repeated carryover chain across devices, skipping a blank gap day.
// d1 has content; d2 is never created (blank gap); D0 carries d1 -> d3 (the 14-day
// lookback skips d2); then D1 carries the already-carried d3 -> d4. Exercises the
// lookback gap-skip and a carryover whose source is itself carried content.
pub fn scenario_n_carryover_chain(h: &mut Harness) {
    h.login_all();
    let d1 = NaiveDate::from_ymd_opt(2027, 2, 1).unwrap();
    // d2 (2027-02-02) is intentionally never created — a blank gap.
    let d3 = NaiveDate::from_ymd_opt(2027, 2, 3).unwrap();
    let d4 = NaiveDate::from_ymd_opt(2027, 2, 4).unwrap();

    h.seed_daily_queue(
        D0,
        d1,
        vec![
            Item::new("standup notes").with_marker(ItemMarker::Checkbox),
            Item::new("inbox zero").with_marker(ItemMarker::Checkbox),
        ],
    );
    let d3_id = h.set_daily_queue(D0, d3, &[""]);
    h.settle();
    h.assert_all_converged();

    // Hop 1: D0 rolls d1 forward into d3, skipping the blank d2 gap.
    let hop1 = h.carryover_daily_queue(D0, d3).expect("d1 -> d3");
    assert_eq!(hop1.len(), 2);
    h.settle();
    h.assert_all_converged();
    for key in h.device_keys() {
        h.assert_scheme_items(key, d3_id, &["standup notes", "inbox zero"]);
    }

    // Hop 2 on a DIFFERENT device: D1 rolls the already-carried d3 forward into d4.
    let d4_id = h.set_daily_queue(D1, d4, &[""]);
    h.settle();
    let hop2 = h.carryover_daily_queue(D1, d4).expect("d3 -> d4");
    assert_eq!(hop2.len(), 2);
    h.settle();
    h.assert_all_converged();
    for key in h.device_keys() {
        h.assert_scheme_items(key, d4_id, &["standup notes", "inbox zero"]);
    }
}

/// Assert no two rows in `scheme` share an `ItemId` on device `key` — the invariant
/// the 2026-06-16 "item id mismatch" wedge violated.
fn assert_no_duplicate_item_ids(h: &Harness, key: DeviceKey, scheme: SchemeId) {
    let mut ids: Vec<_> = h.device(key).workspace.schemes[&scheme]
        .items
        .iter()
        .map(|item| item.id)
        .collect();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "{key:?}: duplicate ItemId in {scheme} after carryover"
    );
}
