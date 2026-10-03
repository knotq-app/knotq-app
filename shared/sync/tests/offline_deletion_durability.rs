//! One property, many interruptions: **a deletion made while a device cannot
//! reach the server survives whatever happens before its next successful sync.**
//!
//! Reported from the field on 2026-10-01: a line deleted while the app showed
//! "offline" came back at the next sync, on every device. A deletion that is
//! dropped is worse than one that fails to sync — the user is told the line is
//! gone, and then it is not.
//!
//! Each test below is the same shape: delete offline, interrupt, sync, and
//! require the line to stay gone everywhere. They differ only in what happens
//! between the delete and the sync, because that is the whole space a real
//! "offline" period can traverse.

mod common;

use common::{DeviceKey, Harness, D0, D1};

/// What a scheme reads as on one device.
fn texts(h: &Harness, device: DeviceKey, scheme: knotq_model::SchemeId) -> Vec<String> {
    h.device(device).scheme_item_texts(scheme)
}

/// Every device agrees, and the deleted line is gone from all of them.
fn assert_deletion_held(h: &Harness, scheme: knotq_model::SchemeId, case: &str) {
    let expected = vec!["keep one".to_string(), "keep two".to_string()];
    for device in h.device_keys() {
        assert_eq!(
            texts(h, device, scheme),
            expected,
            "{case}: {device:?} did not keep the deletion"
        );
    }
}

/// Two devices, one shared scheme, settled. Returns the scheme.
fn settled_scheme(h: &mut Harness) -> knotq_model::SchemeId {
    let scheme = h.add_scheme(
        D0,
        "Offline deletion",
        &["keep one", "delete me", "keep two"],
    );
    h.settle();
    scheme
}

/// The control: nothing goes wrong. If this ever fails, nothing below means
/// anything.
#[test]
fn an_offline_deletion_survives_a_plain_reconnect() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    h.settle();

    assert_deletion_held(&h, scheme, "plain reconnect");
}

/// The app is closed and reopened before it ever reaches the server — the
/// ordinary shape of "I deleted it on the train".
#[test]
fn an_offline_deletion_survives_a_restart_before_the_first_sync() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    h.restart(D0);
    h.settle();

    assert_deletion_held(&h, scheme, "restart before first sync");
}

/// Several failed syncs — what "offline" actually looks like — then one that
/// works.
#[test]
fn an_offline_deletion_survives_repeated_failed_syncs() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    for _ in 0..3 {
        h.reject_next_push_with_code("internal_error");
        let _ = h.try_sync(D0);
    }
    h.settle();

    assert_deletion_held(&h, scheme, "repeated failed syncs");
}

/// The server rejects the push as unparseable and the engine self-heals by
/// reseeding the document from the local CRDT. The reseed must carry the
/// tombstone, not just the surviving lines.
#[test]
fn an_offline_deletion_survives_a_push_reseed() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    h.reject_next_push_with_schema_invalid();
    h.settle();

    assert_deletion_held(&h, scheme, "push reseed");
}

/// The other device edits the same scheme while this one is away. Merging the
/// two must not resurrect the deleted line.
#[test]
fn an_offline_deletion_survives_a_concurrent_edit_elsewhere() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    h.edit_line(D1, scheme, 0, "keep one");
    h.append_line(D1, scheme, "added while away");
    h.settle();

    for device in h.device_keys() {
        let seen = texts(&h, device, scheme);
        assert!(
            !seen.iter().any(|line| line == "delete me"),
            "{device:?}: concurrent edit resurrected the deleted line: {seen:?}"
        );
    }
}

/// The outbound journal is damaged — an interrupted write, the shape
/// `sync-state.json` recovery exists for. The CRDT still holds the tombstone,
/// so the deletion is still knowable; it must be re-expressed rather than
/// dropped.
#[test]
fn an_offline_deletion_survives_a_damaged_sync_journal() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    {
        let state = h.device_mut_for_surgery(D0).local_state_mut();
        state.pending.clear();
        state.document_cursors.clear();
        state.storage_recovery_pending = true;
    }
    h.settle();

    assert_deletion_held(&h, scheme, "damaged sync journal");
}

/// The journal is damaged and nothing marks it — an older build's recovery, or
/// a loader that returned a default without saying so. The pessimistic case: the
/// tombstone is in the CRDT but nothing in the journal tells the engine to look.
///
/// This was `#[ignore]`d as a known gap: with no cursor for the document,
/// nothing could tell this device apart from one that has just signed in, whose
/// local tombstones are pre-join starter noise that must NOT be published — and
/// two attempts to re-offer anyway each cost other data (production fuzz seed
/// 20082 lost an archived folder, chaos seed 11 published a fresh install's
/// starter tombstones onto the account's live rows).
///
/// **It now passes, and not for the reason TODO 0y predicted.** Measured
/// 2026-10-02: it passes with the engine's journal-loss handling unchanged, so
/// what closed it is the integrity proof reaching the document (0x) plus the
/// durable-tombstone recovery path, not a "this device has synced before"
/// witness. 0y records that, because the obvious next step — inferring a lost
/// journal and re-offering on that basis — is the step that lost data twice, and
/// nothing here now demands it.
#[test]
fn an_offline_deletion_survives_an_unmarked_journal_loss() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    {
        let state = h.device_mut_for_surgery(D0).local_state_mut();
        state.pending.clear();
        state.document_cursors.clear();
    }
    h.settle();

    assert_deletion_held(&h, scheme, "unmarked journal loss");
}

/// This scheme's CRDT state file comes back unreadable, so the document is
/// rebuilt from the plain workspace. The plain workspace already has the line
/// removed, so the deletion is still expressible — the question is whether the
/// rebuild publishes it or adopts the server's live copy instead.
#[test]
fn an_offline_deletion_survives_losing_that_schemes_crdt_state() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    h.lose_crdt_state_for_scheme(D0, scheme);
    h.settle();

    assert_deletion_held(&h, scheme, "lost scheme CRDT state");
}

/// A quit during a sync run rewinds that run's pull cursors. A deletion queued
/// before it must not be rewound with them.
#[test]
fn an_offline_deletion_survives_abandoning_an_unlanded_pull() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    {
        let state = h.device_mut_for_surgery(D0).local_state_mut();
        let documents: Vec<_> = state.document_cursors.keys().copied().collect();
        for document in documents {
            state.reset_pull_cursor(document);
        }
        state.unlanded_pulls.clear();
    }
    h.restart(D0);
    h.settle();

    assert_deletion_held(&h, scheme, "abandoned unlanded pull");
}

/// Deleting the last line, which is the shape that leaves an empty scheme —
/// and an empty plain scheme is a shape the pre-pull repair treats specially.
#[test]
fn deleting_every_line_offline_is_not_undone() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "Emptied offline", &["only line"]);
    h.settle();

    h.remove_line(D0, scheme, 0);
    h.restart(D0);
    h.settle();

    for device in h.device_keys() {
        assert!(
            texts(&h, device, scheme).is_empty(),
            "{device:?}: emptying a scheme offline was undone: {:?}",
            texts(&h, device, scheme)
        );
    }
}

/// The outbound queue is gone but the cursors survive — a partial journal loss,
/// a push that cleared the queue against a server that did not keep it, or any
/// bug that drops a queue entry.
///
/// This is the case the "no cursor history" re-offer deliberately does NOT
/// cover: the cursors look perfectly healthy, so nothing suggests re-offering
/// anything. The only thing that can notice is the server's own integrity
/// proof, which compares document hashes on a caught-up pull. It is the
/// backstop for every cause of divergence that leaves no trace locally, so it
/// has to resolve the disagreement in BOTH directions, not just adopt the
/// server's copy.
#[test]
fn an_offline_deletion_survives_losing_only_the_outbound_queue() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    h.remove_line(D0, scheme, 1);
    h.device_mut_for_surgery(D0)
        .local_state_mut()
        .pending
        .clear();
    h.settle();

    assert_deletion_held(&h, scheme, "outbound queue lost, cursors intact");
}

/// The same question for the workspace index rather than a scheme's content:
/// a folder made while offline, then the journal is gone. Folders live in the
/// index document, which is the one document a device must never publish
/// blindly — its content IS the account's identity and tree.
#[test]
fn a_folder_made_offline_survives_an_unmarked_journal_loss() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);

    let folder = h.add_folder(D0, "Made offline");
    h.move_scheme_to_folder(D0, scheme, folder);
    {
        let state = h.device_mut_for_surgery(D0).local_state_mut();
        state.pending.clear();
        state.document_cursors.clear();
    }
    h.settle();

    for device in h.device_keys() {
        assert!(
            h.device(device).has_folder_named("Made offline"),
            "{device:?}: a folder made offline was lost with the journal"
        );
    }
}

// --- the mirror case: the deletion is someone else's ------------------------
//
// Everything above is about this device's own deletion surviving. The same
// property has a second half that the field report did not show but chaos seed
// 332 does: a deletion that already reached the account must not be UNDONE by a
// device whose two halves disagree.
//
// A pull's repair exists to carry local work the CRDT documents do not have yet
// into them. It decides what is local work by diffing the plain scheme files
// against the documents — and that diff cannot tell "I typed this and it has not
// been flushed" from "the pull just told me this row is gone and the plain files
// have not been re-materialized yet". Reading the second as the first puts the
// row back, and then nothing converges: the fleet disagrees about where the row
// lives, each device tombstones the copy it does not believe in, and the account
// can end up holding it nowhere at all. That is chaos 332's "sync lost an item
// no device deleted".
//
// So the repair re-expresses only rows the document has no removal for. The
// plain files are a projection of the documents; their silence about a deletion
// is not evidence against it.

/// Device 1 deletes a line and the account takes it. Device 0 then pulls that
/// deletion while its own document for the scheme has no entry for the row at
/// all — the state a half-written or repopulated CRDT state file leaves — so the
/// row looks like unflushed local work. It must stay deleted.
#[test]
fn a_remote_deletion_is_not_undone_by_a_device_whose_plain_copy_is_ahead() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);
    let doomed = h.device(D0).scheme_items_snapshot(scheme).expect("scheme")[1].id;

    h.remove_line(D1, scheme, 1);
    h.sync(D1);

    // Device 0 has not pulled yet, and its document for this scheme lost the
    // row without recording a tombstone.
    h.drop_item_from_crdt_only(D0, scheme, doomed);
    h.settle();

    assert_deletion_held(&h, scheme, "remote deletion, local plain copy ahead");
    for device in h.device_keys() {
        assert_eq!(
            h.device(device).crdt_scheme_item_texts(scheme),
            vec!["keep one".to_string(), "keep two".to_string()],
            "remote deletion, local plain copy ahead: {device:?}'s own documents kept the row"
        );
    }
}

/// The guard on the test above: refusing to re-express a removed row must not
/// turn the repair off. A line this device really did add, in the very same
/// scheme and the same repair, still has to reach the account.
#[test]
fn a_local_addition_still_lands_when_the_same_scheme_holds_a_remote_deletion() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = settled_scheme(&mut h);
    let doomed = h.device(D0).scheme_items_snapshot(scheme).expect("scheme")[1].id;

    h.remove_line(D1, scheme, 1);
    h.sync(D1);

    // One row the document has no removal for (genuinely unflushed local work)
    // and one it does (the pull's deletion), in the same scheme.
    h.device_mut_for_surgery(D0)
        .direct_append_line_without_crdt(scheme, "typed before the pull");
    h.drop_item_from_crdt_only(D0, scheme, doomed);
    h.settle();

    let expected = vec![
        "keep one".to_string(),
        "keep two".to_string(),
        "typed before the pull".to_string(),
    ];
    for device in h.device_keys() {
        assert_eq!(
            texts(&h, device, scheme),
            expected,
            "{device:?} lost the local addition or kept the remote deletion"
        );
    }
}

/// The case the join witness exists for: the deleted row's id is **derived**, not
/// random, and the journal is gone with nothing marking it.
///
/// A device with no document cursors is indistinguishable from one that has just
/// signed in, so the pre-pull repair deliberately re-asserts only ids it can
/// prove this device authored — a random v4 id. A Daily Queue row's id is derived
/// from its date, so it is byte-identical on every install, and a first-join
/// device's copy of it may be starter content the account deleted long ago. That
/// filter is why `offline_device_join` does not publish a fresh install's starter
/// rows over the account's.
///
/// It would follow that a *genuine* deletion of a derived-id row cannot be
/// recovered through that path — which is the argument TODO 0y was built on.
/// Measured 2026-10-02, it survives anyway: the account's own copy of the
/// document carries the tombstone, and the integrity proof brings the two halves
/// back together without anyone having to infer that the journal was lost. This
/// test exists to keep that true, and to stop the inference being added back on
/// a theory rather than a failure.
#[test]
fn an_offline_deletion_of_a_derived_id_row_survives_an_unmarked_journal_loss() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 7, 15).expect("date");
    let mut h = Harness::new(2);
    h.login_all();
    // A derived (v8) id, the shape every generated row has: a starter line, a
    // carryover's archived row, a daily page. `Item::new` would mint a v4, which
    // the first-join filter already accepts, so the case would not be exercised.
    let mut doomed_row = knotq_model::Item::new("delete me");
    doomed_row.id = knotq_model::ItemId(knotq_model::daily_queue_document_id(date).0);
    let doomed = doomed_row.id;
    assert_eq!(
        doomed.0.get_version_num(),
        8,
        "this test is only meaningful for a derived id"
    );
    let daily = h.seed_daily_queue(
        D0,
        date,
        vec![
            knotq_model::Item::new("keep one"),
            doomed_row,
            knotq_model::Item::new("keep two"),
        ],
    );
    h.record_workspace_change_pub(D0);
    h.settle();

    h.remove_line(D0, daily, 1);
    {
        let state = h.device_mut_for_surgery(D0).local_state_mut();
        state.pending.clear();
        state.document_cursors.clear();
    }
    h.settle();

    let expected = vec!["keep one".to_string(), "keep two".to_string()];
    for device in h.device_keys() {
        assert_eq!(
            texts(&h, device, daily),
            expected,
            "derived-id deletion, unmarked journal loss: {device:?} did not keep it"
        );
    }
}
