//! Edit/delete/archive race + folder-shuffle + zig-zag interleave scenarios (a–d).

use super::super::{Harness, D0, D1};

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
