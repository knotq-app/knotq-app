//! Two-device happy-path convergence through the real batched sync engine.
//!
//! The harness lives in `common`: an in-memory [`common::TestServer`] implementing
//! the production `SyncTransport` over the merged-state model, with devices driven
//! by the actual `batch_pull_and_apply` / `batch_push_pending` engine. No network.
//! The sibling `sync_convergence_scenarios.rs` pushes harder (N devices, wider op
//! set, fuzz).

mod common;

use common::{Harness, D0, D1};

#[test]
fn fresh_signed_in_device_discovers_existing_workspace_and_converges() {
    let mut h = Harness::new(2);
    h.login_all();

    let project = h.add_scheme(D0, "Project", &["base"]);
    h.sync(D0);

    // A second device that has never synced pulls the whole workspace in one batched
    // request and discovers the scheme created elsewhere.
    h.sync(D1);

    h.assert_all_converged();
    h.assert_scheme_active(D1, project);
    h.assert_scheme_items(D1, project, &["base"]);
}

#[test]
fn archived_scheme_stays_archived_across_sync_round_trips() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Archive Me", &["kept content"]);
    h.settle();

    h.archive_scheme(D0, scheme);
    h.settle();

    h.assert_all_converged();
    h.assert_scheme_archived(D0, scheme);
    h.assert_scheme_archived(D1, scheme);
    // Content is retained even though the scheme is out of the sidebar.
    h.assert_scheme_items(D1, scheme, &["kept content"]);
}

#[test]
fn fresh_device_pulls_existing_archived_scheme_with_content() {
    let mut h = Harness::new(2);
    h.login_all();

    let active = h.add_scheme(D0, "Active", &["visible"]);
    let archived = h.add_scheme(D0, "Archived", &["hidden but synced"]);
    h.archive_scheme(D0, archived);
    h.sync(D0);

    // D1 has never synced before. Its first pull must discover both the workspace
    // archive state and the archived scheme's document content.
    h.sync(D1);

    h.assert_all_converged();
    h.assert_scheme_active(D1, active);
    h.assert_scheme_archived(D1, archived);
    h.assert_scheme_items(D1, archived, &["hidden but synced"]);
}

#[test]
fn concurrent_edits_then_archive_converge() {
    let mut h = Harness::new(2);
    h.login_all();

    let plan = h.add_scheme(D0, "Plan", &["base"]);
    h.settle();

    // Both devices append concurrently to the same scheme while offline.
    h.append_line(D0, plan, "from A");
    h.append_line(D1, plan, "from B");
    h.settle();

    h.assert_all_converged();
    h.assert_scheme_items_unordered(D0, plan, &["base", "from A", "from B"]);

    // Then one device archives it; the archive converges across both.
    h.archive_scheme(D1, plan);
    h.settle();

    h.assert_all_converged();
    h.assert_scheme_archived(D0, plan);
    h.assert_scheme_archived(D1, plan);
    h.assert_scheme_items_unordered(D0, plan, &["base", "from A", "from B"]);
    h.assert_scheme_items_unordered(D1, plan, &["base", "from A", "from B"]);
}
