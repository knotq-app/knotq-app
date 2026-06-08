//! Diverse, high-fidelity multi-device convergence scenarios for the batched CRDT
//! sync engine.
//!
//! Every scenario drives the *real* shared engine (`batch_pull_and_apply` +
//! `batch_push_pending`) against an in-memory [`common::TestServer`] that
//! implements the production `SyncTransport` over the merged-state model — one
//! merged Yjs state per document, returned whole on pull. No network is involved.
//! The wider operation set (edit/insert/remove/reorder lines, rename, folders,
//! move, archive, restore, daily queue) plus a deterministic randomized fuzz
//! driver across three devices asserts every replica converges to a byte-identical
//! workspace.

mod common;

use chrono::NaiveDate;
use common::{Harness, Rng, D0, D1, D2};
use knotq_model::{NodeRef, SchemeId};

#[test]
fn three_devices_converge_on_concurrent_distinct_scheme_creation() {
    let mut h = Harness::new(3);
    h.login_all();

    // Each device independently creates its own scheme while offline.
    let a = h.add_scheme(D0, "Alpha", &["a1"]);
    let b = h.add_scheme(D1, "Bravo", &["b1"]);
    let c = h.add_scheme(D2, "Charlie", &["c1"]);

    h.settle();

    h.assert_all_converged();
    for device in h.device_keys() {
        for scheme in [a, b, c] {
            assert!(
                h.device(device).workspace.schemes.contains_key(&scheme),
                "{device:?} lost a concurrently created scheme",
            );
        }
    }
    // All three additions survive in the shared root — no whole-document LWW.
    let root = h.device(D0).root_scheme_ids();
    for scheme in [a, b, c] {
        assert!(root.contains(&scheme), "scheme missing from converged root");
    }
}

#[test]
fn concurrent_same_line_edits_merge_through_full_server_round_trip() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Notes", &["hello"]);
    h.settle();

    // Both devices edit the same single line while offline: one appends, the other
    // prepends. A sequence-CRDT text type keeps both insertions.
    h.edit_line(D0, scheme, 0, "hello!");
    h.edit_line(D1, scheme, 0, "Xhello");
    h.settle();

    h.assert_all_converged();
    let text = h.device(D0).scheme_item_texts(scheme).join("");
    assert!(text.contains('X'), "prepend lost: {text:?}");
    assert!(text.contains('!'), "append lost: {text:?}");
    assert!(text.contains("hello"), "base text lost: {text:?}");
}

#[test]
fn offline_queue_of_many_edits_pushes_in_order_and_converges() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Journal", &["seed"]);
    h.settle();

    // Stack up a long batch of edits on one device with no syncing in between, so
    // they queue as a chain of deltas that must replay in order on the peer.
    for i in 0..25 {
        h.append_line(D0, scheme, &format!("line {i}"));
    }
    h.edit_line(D0, scheme, 0, "seed edited");
    h.settle();

    h.assert_all_converged();
    let texts = h.device(D1).scheme_item_texts(scheme);
    assert_eq!(texts.len(), 26, "{texts:?}");
    assert_eq!(texts[0], "seed edited");
    assert_eq!(texts[25], "line 24");
}

#[test]
fn item_reordering_converges_across_devices() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "List", &["one", "two", "three", "four"]);
    h.settle();

    h.reorder_reverse(D0, scheme);
    h.settle();

    h.assert_all_converged();
    assert_eq!(
        h.device(D1).scheme_item_texts(scheme),
        vec!["four", "three", "two", "one"],
    );
}

#[test]
fn rename_on_one_device_and_content_edit_on_another_both_survive() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Draft", &["body"]);
    h.settle();

    // Rename touches the workspace document; the content edit touches the scheme
    // document. They live in different CRDT docs, so both must survive the merge.
    h.rename_scheme(D0, scheme, "Final");
    h.append_line(D1, scheme, "more body");
    h.settle();

    h.assert_all_converged();
    assert_eq!(h.device(D1).workspace.schemes[&scheme].name, "Final");
    assert_eq!(
        h.device(D0).scheme_item_texts(scheme),
        vec!["body", "more body"],
    );
}

#[test]
fn folder_creation_and_scheme_move_converges() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Movable", &["x"]);
    h.settle();

    let folder = h.add_folder(D0, "Projects");
    h.move_scheme_to_folder(D0, scheme, folder);
    h.settle();

    h.assert_all_converged();
    let peer = h.device(D1);
    assert!(peer.workspace.folders.contains_key(&folder), "folder lost");
    assert!(
        peer.workspace.folders[&folder]
            .children
            .contains(&NodeRef::Scheme(scheme)),
        "scheme did not move into folder on peer",
    );
    assert!(
        !peer.root_scheme_ids().contains(&scheme),
        "scheme still dangling at root after move",
    );
}

#[test]
fn daily_queue_entries_sync_across_devices() {
    let mut h = Harness::new(2);
    h.login_all();
    let date = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
    let daily = h.set_daily_queue(D0, date, &["carry over", "todo"]);
    h.settle();

    h.assert_all_converged();
    let peer = h.device(D1);
    assert_eq!(peer.workspace.daily_queue_scheme_id(date), Some(daily));
    assert_eq!(
        peer.scheme_item_texts(daily),
        vec!["carry over", "todo"],
        "daily queue contents did not sync",
    );
    // Daily queue schemes never appear in the sidebar tree.
    assert!(!peer.root_scheme_ids().contains(&daily));
}

#[test]
fn archive_on_one_device_while_other_edits_content_converges_archived() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Doomed", &["keep"]);
    h.settle();

    // One device archives the scheme; the other concurrently edits its content
    // before learning of the archive. The archive (workspace doc) and the edit
    // (scheme doc) are independent, so the edit is retained but the scheme stays
    // archived and out of the sidebar.
    h.archive_scheme(D0, scheme);
    h.append_line(D1, scheme, "late edit");
    h.settle();

    h.assert_all_converged();
    for device in h.device_keys() {
        let d = h.device(device);
        assert!(
            d.workspace.is_scheme_deleted(scheme),
            "{device:?} un-archived"
        );
        assert!(
            !d.root_scheme_ids().contains(&scheme),
            "{device:?} sidebar leak"
        );
    }
}

#[test]
fn archive_then_restore_round_trips_through_sidebar() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Boomerang", &["content"]);
    h.settle();

    h.archive_scheme(D0, scheme);
    h.settle();
    h.assert_all_converged();
    assert!(h.device(D1).workspace.is_scheme_deleted(scheme));

    h.restore_scheme(D1, scheme);
    h.settle();

    h.assert_all_converged();
    for device in h.device_keys() {
        let d = h.device(device);
        assert!(
            !d.workspace.is_scheme_deleted(scheme),
            "{device:?} still archived"
        );
        assert!(
            d.root_scheme_ids().contains(&scheme),
            "{device:?} not back in sidebar"
        );
    }
}

#[test]
fn late_device_catches_up_through_merged_state() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "History", &["v1"]);
    h.settle();

    // Build up a deep history on D0 while D1 stays offline, so D1's pull cursor
    // lags far behind. In the merged-state model there is no delta log to replay
    // or compact: each push folds into a single head state.
    for i in 0..15 {
        h.append_line(D0, scheme, &format!("rev {i}"));
        h.sync(D0);
    }
    h.rename_scheme(D0, scheme, "Compacted");
    h.sync(D0);

    // One batched pull hands the long-offline device the current merged state and
    // it converges — no forced-snapshot path, no lost history.
    h.settle();

    h.assert_all_converged();
    assert_eq!(h.device(D1).workspace.schemes[&scheme].name, "Compacted");
    let texts = h.device(D1).scheme_item_texts(scheme);
    assert_eq!(texts.first().map(String::as_str), Some("v1"));
    assert_eq!(
        texts.len(),
        16,
        "late device lost history through merged state: {texts:?}"
    );
}

#[test]
fn interleaved_edits_from_three_devices_converge() {
    let mut h = Harness::new(3);
    h.login_all();
    let scheme = h.add_scheme(D0, "Mixed", &["base"]);
    h.settle();

    h.append_line(D0, scheme, "from 0");
    h.sync(D0);
    // New edits land on two other devices that already share the base state.
    h.append_line(D1, scheme, "from 1");
    h.append_line(D2, scheme, "from 2");
    h.settle();

    h.assert_all_converged();
    let mut texts = h.device(D0).scheme_item_texts(scheme);
    texts.sort();
    assert_eq!(texts, vec!["base", "from 0", "from 1", "from 2"]);
}

#[test]
fn randomized_multi_device_operations_converge() {
    // Several seeds so a single lucky/unlucky interleaving cannot hide a bug.
    for seed in [1u64, 7, 42, 123, 2024, 99991] {
        run_randomized_scenario(seed);
    }
}

fn run_randomized_scenario(seed: u64) {
    let mut rng = Rng::new(seed);
    let mut h = Harness::new(3);
    h.login_all();

    // Seed a handful of schemes from device 0 and propagate so every device shares
    // the same starting set of documents.
    let mut schemes: Vec<SchemeId> = Vec::new();
    for i in 0..4 {
        schemes.push(h.add_scheme(D0, &format!("S{i}"), &["seed"]));
    }
    h.settle();

    let devices = h.device_keys();
    for _ in 0..120 {
        let device = devices[rng.below(devices.len() as u64) as usize];
        let scheme = schemes[rng.below(schemes.len() as u64) as usize];
        match rng.below(8) {
            0 => {
                let text = format!("append-{}", rng.below(1000));
                h.append_line(device, scheme, &text);
            }
            1 => {
                let len = h.device(device).workspace.schemes[&scheme].items.len();
                if len > 0 {
                    let idx = rng.below(len as u64) as usize;
                    h.edit_line(device, scheme, idx, &format!("edit-{}", rng.below(1000)));
                }
            }
            2 => {
                let len = h.device(device).workspace.schemes[&scheme].items.len();
                let idx = rng.below((len + 1) as u64) as usize;
                h.insert_line(device, scheme, idx, &format!("ins-{}", rng.below(1000)));
            }
            3 => {
                let len = h.device(device).workspace.schemes[&scheme].items.len();
                if len > 1 {
                    let idx = rng.below(len as u64) as usize;
                    h.remove_line(device, scheme, idx);
                }
            }
            4 => h.reorder_reverse(device, scheme),
            5 => h.rename_scheme(device, scheme, &format!("R{}", rng.below(1000))),
            6 => {
                // Archive/restore toggle keeps the sidebar churning.
                if h.device(device).workspace.is_scheme_deleted(scheme) {
                    h.restore_scheme(device, scheme);
                } else {
                    h.archive_scheme(device, scheme);
                }
            }
            _ => h.sync(device),
        }

        // Randomly let some device sync mid-stream so changes interleave.
        if rng.below(3) == 0 {
            let syncer = devices[rng.below(devices.len() as u64) as usize];
            h.sync(syncer);
        }
    }

    h.settle();
    h.assert_all_converged_with_context(seed);

    // Every scheme that was ever created still exists on every device (whether
    // active or archived) — nothing was silently dropped.
    for device in &devices {
        for scheme in &schemes {
            assert!(
                h.device(*device).workspace.schemes.contains_key(scheme),
                "seed {seed}: {device:?} lost scheme {scheme}",
            );
        }
    }
}
