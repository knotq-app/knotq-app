//! Daily-queue "roll over from yesterday" (carryover) scenarios (m/m2–m5/n).
//!
//! Carryover is a single user action that mutates TWO scheme documents at once:
//! every carried row MOVES its ItemId into today (notification identity and
//! cross-device dedupe follow the live item), while the row left on the source day
//! is re-identified with the deterministic displaced id and has its date
//! annotations stripped. That cross-document, id-moving shape is exactly the class
//! of edit that has wedged production sync before (empty/duplicate daily docs,
//! item-id collisions). These scenarios stress it — including that two devices
//! rolling the same day concurrently now CONVERGE to single rows (matching ids
//! dedupe through the CRDT item skeletons) instead of doubling.

use chrono::{NaiveDate, TimeZone, Utc};
use knotq_model::{daily_queue_displaced_item_id, Item, ItemMarker, SchemeId};

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
    let dentist = Item::new("call dentist").with_start(due); // dated -> carried, source stripped
    let dentist_source_id = dentist.id;
    let prev = h.seed_daily_queue(
        D0,
        yesterday,
        vec![
            Item::new("carry me").with_marker(ItemMarker::Checkbox), // incomplete -> carried
            Item::new("finished")
                .with_marker(ItemMarker::Checkbox)
                .done(), // done -> NOT carried
            Item::new("loose note"),                                 // plain -> carried
            dentist,
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

    // The cross-document split: the archived row on yesterday was stripped and
    // re-identified with the displaced id, while the carried row in today keeps
    // BOTH its date and its source ItemId — and both halves reach every device.
    for key in h.device_keys() {
        let archived = find_item(h, key, prev, "call dentist");
        assert!(
            archived.start.is_none(),
            "{key:?}: archived row date should be stripped on yesterday"
        );
        assert_eq!(
            archived.id,
            daily_queue_displaced_item_id(dentist_source_id, yesterday),
            "{key:?}: archived row should carry the deterministic displaced id"
        );
        let carried = find_item(h, key, today_id, "call dentist");
        assert_eq!(
            carried.start,
            Some(due),
            "{key:?}: carried row should keep its date in today"
        );
        assert_eq!(
            carried.id, dentist_source_id,
            "{key:?}: carried row should keep the source item id"
        );
    }
}

// Scenario m2 — Both devices roll the same yesterday into one SHARED (synced) today.
// Carried rows keep their SOURCE ids and the displaced archive ids are
// deterministic, so the two concurrent carries encode matching item skeletons on
// both documents and the merge collapses them: ONE row per task in today, ONE
// archived row per task on yesterday — no row doubling, and the identical
// concurrent fills merge to clean (undoubled) text. Hard requirement:
// convergence, no crdt_schema_invalid wedge, and no ItemId collision.
pub fn scenario_m2_carryover_concurrent_shared_today(h: &mut Harness) {
    h.login_all();
    let yesterday = NaiveDate::from_ymd_opt(2026, 12, 8).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 12, 9).unwrap();

    let sources = vec![
        Item::new("task A").with_marker(ItemMarker::Checkbox),
        Item::new("task B").with_marker(ItemMarker::Checkbox),
        Item::new("task C").with_marker(ItemMarker::Checkbox),
    ];
    let source_ids: Vec<_> = sources.iter().map(|item| item.id).collect();
    let prev = h.seed_daily_queue(D0, yesterday, sources);
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

    // Matching carried ids collapse the concurrent carries: 3 rows in today (the
    // source ids), 3 archived rows on yesterday (the displaced ids).
    for key in h.device_keys() {
        h.assert_scheme_items_unordered(key, today_id, &["task A", "task B", "task C"]);
        h.assert_scheme_items_unordered(key, prev, &["task A", "task B", "task C"]);
        let today_ids: Vec<_> = h.device(key).workspace.schemes[&today_id]
            .items
            .iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(
            today_ids.len(),
            3,
            "{key:?}: concurrent carries of the same rows must collapse to one row each"
        );
        for source_id in &source_ids {
            assert!(
                today_ids.contains(source_id),
                "{key:?}: carried row must keep source id {source_id}"
            );
        }
        let prev_ids: Vec<_> = h.device(key).workspace.schemes[&prev]
            .items
            .iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(
            prev_ids.len(),
            3,
            "{key:?}: concurrent displacement must converge to one archived row each"
        );
        for source_id in &source_ids {
            assert!(
                prev_ids.contains(&daily_queue_displaced_item_id(*source_id, yesterday)),
                "{key:?}: yesterday must hold the deterministic displaced id for {source_id}"
            );
        }
        assert_no_duplicate_item_ids(h, key, today_id);
        assert_no_duplicate_item_ids(h, key, prev);
        assert!(
            h.device(key).is_fully_pushed(),
            "{key:?}: push queue must drain after concurrent carryover"
        );
    }
}

// Scenario m3 — Both devices independently OPEN today offline (same deterministic
// daily SchemeId, but different placeholder ItemIds) and both roll over. Carried
// rows keep their source ids, so even with two distinct placeholders (each device
// deletes only its own) the carried rows collapse to one per task. This also
// proves the deterministic daily document keeps independent creations on ONE doc
// rather than splitting content (the 2026-06-11 empty-daily-doc wedge class).
pub fn scenario_m3_carryover_concurrent_independent_today(h: &mut Harness) {
    h.login_all();
    let yesterday = NaiveDate::from_ymd_opt(2026, 12, 15).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 12, 16).unwrap();

    let sources = vec![
        Item::new("task A").with_marker(ItemMarker::Checkbox),
        Item::new("task B").with_marker(ItemMarker::Checkbox),
    ];
    let source_ids: Vec<_> = sources.iter().map(|item| item.id).collect();
    h.seed_daily_queue(D0, yesterday, sources);
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

    // Matching carried ids collapse both carries to one row per task, and each
    // device's placeholder was deleted by its own carry: exactly {A, B}.
    for key in h.device_keys() {
        h.assert_scheme_items_unordered(key, today_id, &["task A", "task B"]);
        let today_ids: Vec<_> = h.device(key).workspace.schemes[&today_id]
            .items
            .iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(
            today_ids.len(),
            2,
            "{key:?}: independent concurrent carries must collapse to one row each"
        );
        for source_id in &source_ids {
            assert!(
                today_ids.contains(source_id),
                "{key:?}: carried row must keep source id {source_id}"
            );
        }
        assert_no_duplicate_item_ids(h, key, today_id);
        assert!(
            h.device(key).is_fully_pushed(),
            "{key:?}: push queue must drain after independent carryover"
        );
    }
}

// Scenario m4 — D0 rolls yesterday forward while D1 keeps editing yesterday
// concurrently. The roll MOVES each row's id into today and re-identifies the
// archived copy on yesterday, so D1's concurrent edit to a rolled row targets the
// (now tombstoned) source container and is superseded: the archived rows keep the
// snapshot D0 rolled. That is the accepted tradeoff of id-moving carryover — the
// live row is in today now, and yesterday is an archive. D1's concurrent NEW row
// on yesterday still converges in.
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
        // Yesterday holds the archived (displaced) snapshot rows plus D1's new row;
        // D1's edit to the rolled row was superseded by the roll.
        let texts = h.device(key).scheme_item_texts(prev);
        assert_eq!(
            texts.len(),
            3,
            "{key:?}: yesterday should hold 2 archived rows + D1's new row: {texts:?}"
        );
        for expected in ["ship release", "write changelog", "tag the build"] {
            assert!(
                texts.iter().any(|text| text == expected),
                "{key:?}: yesterday is missing {expected:?}: {texts:?}"
            );
        }
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
// lookback gap-skip and a carryover whose source is itself carried content — and
// that the LIVE ItemId (notification identity) rides the whole chain unchanged
// across devices, leaving a distinct per-day displaced id behind on each hop.
pub fn scenario_n_carryover_chain(h: &mut Harness) {
    h.login_all();
    let d1 = NaiveDate::from_ymd_opt(2027, 2, 1).unwrap();
    // d2 (2027-02-02) is intentionally never created — a blank gap.
    let d3 = NaiveDate::from_ymd_opt(2027, 2, 3).unwrap();
    let d4 = NaiveDate::from_ymd_opt(2027, 2, 4).unwrap();

    let sources = vec![
        Item::new("standup notes").with_marker(ItemMarker::Checkbox),
        Item::new("inbox zero").with_marker(ItemMarker::Checkbox),
    ];
    let source_ids: Vec<_> = sources.iter().map(|item| item.id).collect();
    let d1_id = h.seed_daily_queue(D0, d1, sources);
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
        assert_carried_and_displaced(h, key, d3_id, d1_id, &source_ids, d1);
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
        // The SAME source ids carried again — the id (and with it the "daily"
        // notification key) follows the live row across the whole chain, while
        // d3's archive got its own date-scoped displaced ids.
        assert_carried_and_displaced(h, key, d4_id, d3_id, &source_ids, d3);
    }
}

/// Assert the id-transfer contract for one hop on device `key`: every id in
/// `source_ids` lives ON in `today` (the carried rows), while `prev` instead
/// holds the deterministic displaced id derived from (source, `prev_date`).
fn assert_carried_and_displaced(
    h: &Harness,
    key: DeviceKey,
    today: SchemeId,
    prev: SchemeId,
    source_ids: &[knotq_model::ItemId],
    prev_date: NaiveDate,
) {
    let ids_in = |scheme: SchemeId| -> Vec<_> {
        h.device(key).workspace.schemes[&scheme]
            .items
            .iter()
            .map(|item| item.id)
            .collect()
    };
    let today_ids = ids_in(today);
    let prev_ids = ids_in(prev);
    for source_id in source_ids {
        assert!(
            today_ids.contains(source_id),
            "{key:?}: carried row must keep source id {source_id} in {today}"
        );
        assert!(
            !prev_ids.contains(source_id),
            "{key:?}: source id {source_id} must have MOVED off {prev}"
        );
        assert!(
            prev_ids.contains(&daily_queue_displaced_item_id(*source_id, prev_date)),
            "{key:?}: {prev} must hold the displaced id for {source_id}@{prev_date}"
        );
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
