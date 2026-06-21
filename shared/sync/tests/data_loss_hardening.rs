//! Data-loss hardening scenarios for the shared sync engine.
//!
//! These are adversarial, in-memory ([`TestServer`]) tests whose single charter is:
//! **no committed edit may ever be silently lost**. They complement the
//! convergence-focused `hard_scenarios.rs` and the production-wedge reproductions in
//! `sync_wedge_regressions.rs` by asserting on *content completeness* (every line
//! that was written reaches every device), not merely that devices agree.
//!
//! The boundaries exercised here:
//!   - Concurrent writes from 3 devices to one scheme — every write survives.
//!   - A push rejection + self-heal must preserve ALL content, not just drain the
//!     queue (a heal that reseeds a stale/partial snapshot would silently drop data).
//!   - A concurrent delete must drop ONLY its target, never collaterally lose
//!     unrelated schemes created on another device in the same window.
//!   - A long-offline device that exceeds BOTH push batch caps
//!     (`PUSH_MAX_DOCUMENTS_PER_REQUEST` documents and
//!     `PUSH_MAX_UPDATES_PER_DOCUMENT` updates/doc) loses nothing across the
//!     multi-batch, multi-request push.

mod common;

use common::{Harness, D0, D1, D2};

// ---------------------------------------------------------------------------
// No lost writes under three-way concurrent appends to one scheme
// ---------------------------------------------------------------------------

/// Three devices append distinct lines to the SAME scheme while syncs interleave.
/// Every appended line must be present on every device afterwards — a count-exact
/// guarantee, so a dropped or clobbered concurrent insert fails the test rather
/// than hiding behind a mere "they converged" check.
#[test]
fn concurrent_appends_three_devices_no_lost_writes() {
    let mut h = Harness::new(3);
    h.login_all();

    let scheme = h.add_scheme(D0, "Shared Doc", &["seed"]);
    h.settle();

    const PER_DEVICE: usize = 12;
    let devices = [D0, D1, D2];

    // Interleave offline appends and syncs so concurrent insert positions collide.
    for round in 0..PER_DEVICE {
        for &device in &devices {
            h.append_line(device, scheme, &format!("{device:?}-line-{round}"));
        }
        // Sync one device each round to force merges mid-stream (not a clean
        // "everyone edits then everyone syncs" path).
        h.sync(devices[round % devices.len()]);
    }

    h.settle();
    h.assert_all_converged();

    // Every line each device wrote must survive on every device.
    for &observer in &devices {
        let texts = h.device(observer).scheme_item_texts(scheme);
        for &writer in &devices {
            for round in 0..PER_DEVICE {
                let expected = format!("{writer:?}-line-{round}");
                assert!(
                    texts.contains(&expected),
                    "{observer:?} lost write {expected:?}; have {texts:?}"
                );
            }
        }
        // Exact count: seed + 3 devices * PER_DEVICE, no dupes, no drops.
        assert_eq!(
            texts.len(),
            1 + devices.len() * PER_DEVICE,
            "{observer:?}: unexpected item count (lost or duplicated writes)"
        );
    }
}

// ---------------------------------------------------------------------------
// Self-heal after a push rejection must not lose content
// ---------------------------------------------------------------------------

/// A forced `crdt_schema_invalid` rejection mid-sync triggers the reseed-and-retry
/// self-heal. The existing wedge tests assert the queue drains; this one asserts the
/// stronger property that the heal preserves EVERY line — a heal that reseeded an
/// empty or partial snapshot would drain the queue while silently losing data. The
/// second device must materialize the full content from the reseeded server state.
#[test]
fn self_heal_after_rejection_preserves_all_content() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Healable Plan", &["v0"]);
    h.sync(D0);
    h.sync(D1); // D1 learns the scheme exists.

    // Accumulate a body of distinct content offline on D0.
    for i in 0..20 {
        h.append_line(D0, scheme, &format!("payload-{i:02}"));
    }

    // Force the next push to be rejected; the engine must reseed a full snapshot
    // from its persistent CRDT and retry within the same sync call.
    h.reject_next_push_with_schema_invalid();
    h.try_sync(D0)
        .expect("engine must self-heal from crdt_schema_invalid and return Ok");
    assert!(
        h.device(D0).is_fully_pushed(),
        "D0 queue must drain after self-heal; {} remain",
        h.device(D0).pending_count()
    );

    h.settle();
    h.assert_all_converged();

    // The heal must have preserved the seed plus all 20 payload lines on BOTH
    // devices — nothing dropped by the reseed.
    for key in h.device_keys() {
        let texts = h.device(key).scheme_item_texts(scheme);
        assert!(texts.contains(&"v0".to_string()), "{key:?}: lost seed line");
        for i in 0..20 {
            let expected = format!("payload-{i:02}");
            assert!(
                texts.contains(&expected),
                "{key:?}: self-heal lost {expected:?}; have {texts:?}"
            );
        }
        assert_eq!(texts.len(), 21, "{key:?}: wrong item count after self-heal");
    }
}

// ---------------------------------------------------------------------------
// A concurrent delete must not collaterally drop unrelated new schemes
// ---------------------------------------------------------------------------

/// D0 permanently deletes scheme X while D1, concurrently and offline, creates two
/// brand-new schemes and fills them with content. After convergence X is gone on
/// both devices, but Y and Z — created in the same window — must survive intact.
/// This guards against a workspace-index merge that drops unrelated concurrent
/// creations along with the deletion (a real data-loss failure mode for the
/// shared workspace document).
#[test]
fn concurrent_delete_does_not_drop_unrelated_new_schemes() {
    let mut h = Harness::new(2);
    h.login_all();

    let x = h.add_scheme(D0, "Doomed", &["x-content"]);
    h.settle();

    // D0 archives + permanently deletes X offline.
    h.archive_scheme(D0, x);
    h.delete_scheme(D0, x);

    // D1, concurrently and offline, creates two unrelated schemes with content.
    let y = h.add_scheme(D1, "Survivor Y", &["y0"]);
    let z = h.add_scheme(D1, "Survivor Z", &["z0"]);
    for i in 0..5 {
        h.append_line(D1, y, &format!("y-line-{i}"));
        h.append_line(D1, z, &format!("z-line-{i}"));
    }

    // Adversarial sync order: deletion lands first, then the concurrent creations.
    h.sync(D0);
    h.sync(D1);
    h.sync(D0);
    h.settle();
    h.assert_all_converged();

    // The delete affected only its target.
    h.assert_scheme_absent(D0, x);
    h.assert_scheme_absent(D1, x);

    // Both new schemes and all their content survived on both devices.
    for key in h.device_keys() {
        assert!(
            h.device(key).workspace.schemes.contains_key(&y),
            "{key:?}: unrelated scheme Y was collaterally dropped"
        );
        assert!(
            h.device(key).workspace.schemes.contains_key(&z),
            "{key:?}: unrelated scheme Z was collaterally dropped"
        );
        let y_texts = h.device(key).scheme_item_texts(y);
        let z_texts = h.device(key).scheme_item_texts(z);
        for i in 0..5 {
            assert!(
                y_texts.contains(&format!("y-line-{i}")),
                "{key:?}: lost Y content y-line-{i}; have {y_texts:?}"
            );
            assert!(
                z_texts.contains(&format!("z-line-{i}")),
                "{key:?}: lost Z content z-line-{i}; have {z_texts:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Long offline run exceeding BOTH push batch caps loses nothing
// ---------------------------------------------------------------------------

/// A device goes offline long enough to create more than `PUSH_MAX_DOCUMENTS_PER_REQUEST`
/// scheme documents AND to pile more than `PUSH_MAX_UPDATES_PER_DOCUMENT` edits onto a
/// single document. Draining that requires multiple push requests (document paging)
/// and multiple per-document batches. Every scheme and every edit must reach the
/// second device — nothing may be lost at a batch boundary.
#[test]
fn long_offline_exceeds_batch_caps_no_loss() {
    use knotq_sync::{PUSH_MAX_DOCUMENTS_PER_REQUEST, PUSH_MAX_UPDATES_PER_DOCUMENT};

    let mut h = Harness::new(2);
    h.login_all();

    // More documents than fit in one push request.
    let scheme_count = PUSH_MAX_DOCUMENTS_PER_REQUEST + 5;
    let mut schemes = Vec::with_capacity(scheme_count);
    for i in 0..scheme_count {
        schemes.push(h.add_scheme(D0, &format!("offline-{i:03}"), &[&format!("seed-{i:03}")]));
    }

    // Pile more edits than fit in one per-document batch onto the first scheme, so
    // that document alone needs several batches.
    let hot = schemes[0];
    let hot_edits = PUSH_MAX_UPDATES_PER_DOCUMENT * 2 + 7;
    for i in 0..hot_edits {
        h.append_line(D0, hot, &format!("hot-{i:03}"));
    }

    // Drain the whole backlog. settle() runs sync rounds until the devices agree.
    h.settle();
    h.assert_all_converged();
    assert!(
        h.device(D0).is_fully_pushed(),
        "D0 queue must fully drain across multi-batch push; {} remain",
        h.device(D0).pending_count()
    );

    // D1 must see EVERY document...
    for (i, &s) in schemes.iter().enumerate() {
        assert!(
            h.device(D1).workspace.schemes.contains_key(&s),
            "D1 missing offline document #{i} (lost past the {PUSH_MAX_DOCUMENTS_PER_REQUEST}-doc batch cap)"
        );
    }
    // ...and EVERY edit on the hot document.
    let hot_texts = h.device(D1).scheme_item_texts(hot);
    for i in 0..hot_edits {
        let expected = format!("hot-{i:03}");
        assert!(
            hot_texts.contains(&expected),
            "D1 lost hot-doc edit {expected:?} past the {PUSH_MAX_UPDATES_PER_DOCUMENT}-update batch cap"
        );
    }
    // Exact count on the hot doc: seed + all hot edits.
    assert_eq!(
        hot_texts.len(),
        1 + hot_edits,
        "D1: wrong hot-doc item count (lost or duplicated edits at a batch boundary)"
    );
}
