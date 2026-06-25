//! Daily-queue "roll over from yesterday" (carryover) scenarios (m/m2–m5/n).
//!
//! Carryover is a single user action that mutates TWO scheme documents at once —
//! the source day (date annotations stripped) and today (carried clones inserted) —
//! while minting a FRESH ItemId for every carried row. That cross-document, fresh-id
//! shape is exactly the class of edit that has wedged production sync before
//! (empty/duplicate daily docs, item-id collisions). These scenarios stress it.

use chrono::{NaiveDate, TimeZone, Utc};
use knotq_model::{Item, ItemMarker, SchemeId};

use super::super::{DeviceKey, Harness, D0, D1};

/// Find the (first) item with `text` in `scheme` on device `key`.
pub(super) fn find_item<'a>(
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
            Item::new("finished")
                .with_marker(ItemMarker::Checkbox)
                .done(), // done -> NOT carried
            Item::new("loose note"),                                 // plain -> carried
            Item::new("call dentist").with_start(due), // dated -> carried, source stripped
        ],
    );
    // A freshly opened today is a single blank placeholder row.
    let today_id = h.set_daily_queue(D0, today, &[""]);
    h.settle();
    h.assert_all_converged();

    // Roll yesterday's open work into today.
    let carried = h
        .carryover_daily_queue(D0, today)
        .expect("something to carry");
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
    assert_eq!(
        today_id, today_id_d1,
        "daily SchemeId must be deterministic"
    );

    h.carryover_daily_queue(D0, today).expect("D0 carries");
    h.carryover_daily_queue(D1, today).expect("D1 carries");

    h.sync(D0);
    h.sync(D1);
    h.sync(D0);
    h.settle();
    h.assert_all_converged();

    // Two independent placeholders => every row duplicates: {A x2, B x2}.
    for key in h.device_keys() {
        h.assert_scheme_items_unordered(key, today_id, &["task A", "task A", "task B", "task B"]);
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
pub(super) fn assert_no_duplicate_item_ids(h: &Harness, key: DeviceKey, scheme: SchemeId) {
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
