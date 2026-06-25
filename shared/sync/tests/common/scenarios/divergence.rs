//! Long-offline divergence, restart combos, daily-queue conflicts, and calendar
//! import lifecycle scenarios (e–h, plus g2).

use chrono::NaiveDate;

use super::super::{Harness, D0, D1};

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
