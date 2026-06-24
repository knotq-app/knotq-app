//! Default-case correctness guards: concurrent *content* editing must preserve every
//! edit (AB/BA), never silently drop one. These protect the common case so the deeper
//! deterministic-creation fix (which dedupes concurrent *container* creation) cannot
//! regress normal concurrent editing. Convergence alone is NOT enough — devices could
//! agree on a lossy state — so these assert content preservation, not just agreement.

mod common;

use chrono::NaiveDate;
use common::{DeviceKey, Harness, D0, D1, D2};

fn line_texts(h: &Harness, key: DeviceKey, name: &str) -> Vec<String> {
    h.device(key)
        .workspace
        .schemes
        .values()
        .find(|s| s.name == name)
        .map(|s| s.items.iter().map(|i| i.text()).collect())
        .unwrap_or_default()
}

/// The exact scenario raised: two devices each insert a *different* line at the *same*
/// position in a shared scheme, concurrently (no sync between). After merge BOTH lines
/// must survive (order may be AB or BA) — neither is discarded.
#[test]
fn concurrent_inserts_at_same_position_keep_both() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Doc", &["start"]);
    h.sync(D0);
    h.sync(D1); // both now share the same origin for this scheme

    // Concurrent edits at the same index, with NO sync in between.
    h.insert_line(D0, scheme, 1, "AAA");
    h.insert_line(D1, scheme, 1, "BBB");

    h.settle();
    h.assert_all_converged();

    let texts = line_texts(&h, D0, "Doc");
    assert!(texts.contains(&"AAA".to_string()), "D0's insert lost: {texts:?}");
    assert!(texts.contains(&"BBB".to_string()), "D1's insert lost: {texts:?}");
    assert!(texts.contains(&"start".to_string()), "seed line lost: {texts:?}");
    assert_eq!(texts.len(), 3, "expected exactly start+AAA+BBB, got {texts:?}");
}

/// Three devices each append a distinct line concurrently — all three must survive.
#[test]
fn concurrent_appends_from_three_devices_all_survive() {
    let mut h = Harness::new(3);
    h.login_all();
    let scheme = h.add_scheme(D0, "Doc", &["seed"]);
    h.sync(D0);
    h.sync(D1);
    h.sync(D2);

    h.append_line(D0, scheme, "from-D0");
    h.append_line(D1, scheme, "from-D1");
    h.append_line(D2, scheme, "from-D2");

    h.settle();
    h.assert_all_converged();

    let texts = line_texts(&h, D0, "Doc");
    for expected in ["seed", "from-D0", "from-D1", "from-D2"] {
        assert!(texts.contains(&expected.to_string()), "{expected} lost: {texts:?}");
    }
    assert_eq!(texts.len(), 4, "no extra/duplicated lines: {texts:?}");
}

/// Concurrent INDEPENDENT creation of the same daily-queue date (deterministic id) on
/// three devices that haven't synced — the carryover-collision scenario. With Bug A's
/// deterministic skeleton the container dedupes, and the three distinct items must all
/// survive (union), converging across devices.
#[test]
fn concurrent_independent_daily_queue_creation_converges() {
    let mut h = Harness::new(3);
    h.login_all();
    let date = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
    h.device_mut_for_surgery(D0).set_daily_queue(date, &["d0"]);
    h.device_mut_for_surgery(D1).set_daily_queue(date, &["d1"]);
    h.device_mut_for_surgery(D2).set_daily_queue(date, &["d2"]);
    h.settle();
    h.assert_all_converged();
    let texts = line_texts(&h, D0, "Daily");
    for expected in ["d0", "d1", "d2"] {
        assert!(texts.contains(&expected.to_string()), "{expected} lost: {texts:?}");
    }
}

/// Diagnostic: an edit to an ARCHIVED scheme must still propagate to other devices
/// (archived schemes are hidden, not deleted — their content must stay consistent).
/// Single account, to isolate from account-switch effects.
#[test]
fn edits_to_archived_scheme_propagate_single_account() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Doc", &["a"]);
    h.sync(D0);
    h.sync(D1);
    h.archive_scheme(D0, scheme);
    h.settle();
    h.append_line(D0, scheme, "after-archive");
    h.settle();
    h.assert_all_converged();
    let texts = line_texts(&h, D1, "Doc");
    assert!(
        texts.contains(&"after-archive".to_string()),
        "D1 missing the edit made to the archived scheme: {texts:?}"
    );
}

/// Many concurrent inserts at the same position across two devices: every distinct
/// line must appear exactly once (no loss, no duplication).
#[test]
fn many_concurrent_same_position_inserts_no_loss_no_dup() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Doc", &["seed"]);
    h.sync(D0);
    h.sync(D1);

    for i in 0..10 {
        h.insert_line(D0, scheme, 1, &format!("d0-{i}"));
        h.insert_line(D1, scheme, 1, &format!("d1-{i}"));
    }

    h.settle();
    h.assert_all_converged();

    let texts = line_texts(&h, D0, "Doc");
    for i in 0..10 {
        assert_eq!(
            texts.iter().filter(|t| *t == &format!("d0-{i}")).count(),
            1,
            "d0-{i} not present exactly once: {texts:?}"
        );
        assert_eq!(
            texts.iter().filter(|t| *t == &format!("d1-{i}")).count(),
            1,
            "d1-{i} not present exactly once: {texts:?}"
        );
    }
}
