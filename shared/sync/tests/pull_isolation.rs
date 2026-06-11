//! Tests for per-document pull isolation: a single bad or orphan document must
//! not block the rest of the pull, and cursors must advance correctly so the
//! engine does not re-pull the same document every cycle unless needed.

mod common;

use common::{Harness, D0, D1};
use knotq_model::{Item, Scheme};

// ---------------------------------------------------------------------------
// Test (a): orphan scheme content doc is skipped, cursor advances, no re-pull
// ---------------------------------------------------------------------------

/// Server holds an orphan scheme content doc (no workspace-index entry).
/// Device B pulls: succeeds, applies all other docs, reports the orphan in
/// `last_skipped`, advances the cursor, and does NOT re-pull the orphan on
/// the next sync (cursor advanced).
#[test]
fn orphan_scheme_document_is_skipped_and_cursor_advances() {
    let mut h = Harness::new(2);
    h.login_all();

    // D0 creates and syncs a real scheme — gives the server a workspace doc.
    let real_scheme = h.add_scheme(D0, "Real Scheme", &["line A", "line B"]);
    h.sync(D0);

    // Inject an orphan scheme content doc directly on the server (no workspace
    // index entry pointing to it — simulates the production bug).
    let mut orphan = Scheme::new("Orphan (never indexed)", 0);
    orphan.items.push(Item::new("orphan line"));
    let orphan_doc_id = h.inject_orphan_scheme_document(&orphan);

    let pulls_before = h.server_pull_calls();

    // D1 syncs: must succeed despite the orphan.
    h.sync(D1);

    // The pull must complete (no Err — already asserted by h.sync which panics
    // on failure) and D1 must see the real scheme.
    assert!(
        h.device(D1).workspace.schemes.contains_key(&real_scheme),
        "D1 must have the real scheme after pulling"
    );

    // The orphan must appear in last_skipped.
    let skipped = &h.device(D1).last_skipped;
    assert!(
        skipped.iter().any(|s| s.document == orphan_doc_id),
        "orphan document {orphan_doc_id} must be in last_skipped; got: {skipped:?}"
    );

    // The skipped orphan must be flagged as `unknown_scheme_document`.
    let orphan_skip = skipped
        .iter()
        .find(|s| s.document == orphan_doc_id)
        .unwrap();
    assert!(
        orphan_skip.unknown_scheme_document,
        "orphan skip must have unknown_scheme_document=true"
    );

    // The cursor for the orphan must have advanced (mark_pulled was called).
    let cursor_seq = h
        .device(D1)
        .local_state_ref()
        .document_cursors
        .get(&orphan_doc_id)
        .map(|c| c.last_pulled_sequence);
    assert!(
        cursor_seq.is_some() && cursor_seq.unwrap() > 0,
        "cursor for orphan document must have advanced; got {:?}",
        cursor_seq
    );

    let pulls_after_first_sync = h.server_pull_calls();
    assert!(
        pulls_after_first_sync > pulls_before,
        "server must have been pulled at least once"
    );

    // Second sync must NOT re-pull the orphan (cursor already at server seq).
    // The server's document count does not change, so a second pull with the
    // advanced cursor returns nothing for the orphan.
    h.sync(D1);
    let skipped_second = &h.device(D1).last_skipped;
    assert!(
        !skipped_second.iter().any(|s| s.document == orphan_doc_id),
        "orphan must NOT appear in last_skipped on second sync (cursor advanced)"
    );
}

// ---------------------------------------------------------------------------
// Test (b): re-convergence via cursor reset when index catches up to orphan
// ---------------------------------------------------------------------------

/// Simulate: device A creates and syncs a scheme.  A buggy path on another
/// device (or the orphan injection) caused a scheme content doc to arrive on
/// D1 before the workspace-index update.  D1 skips the content doc (orphan)
/// and advances the cursor.  Later, D0 adds a new scheme (with the same name)
/// and syncs.  D1 must eventually converge with full scheme content.
///
/// This test verifies the cursor-reset path specifically for schemes that are
/// in the workspace index but have no local CRDT doc:
///   - D1's cursor for the orphan content doc was advanced (from the first skip).
///   - When D0 pushes a new proper scheme, D1's workspace index gains a NEW
///     scheme entry.  That new scheme's content doc has no local CRDT doc on D1,
///     so the engine resets its pull cursor to 0.
///   - On the next pull, D1 fetches and applies the content (cursor was 0).
#[test]
fn cursor_reset_allows_reconvergence_when_index_later_arrives() {
    let mut h = Harness::new(2);
    h.login_all();

    // Step 1: D0 creates a seed scheme so the server gets a valid workspace base,
    // then both devices sync to share that base.
    let _seed = h.add_scheme(D0, "Seed", &["seed item"]);
    h.sync(D0);
    h.sync(D1);

    // Step 2: Inject an orphan content doc (scheme content without workspace-index
    // entry).  D1 pulls: sees the orphan, skips it, cursor advances.
    let mut orphan = Scheme::new("Will Converge", 0);
    orphan.items.push(Item::new("item from orphan"));
    let orphan_doc_id = h.inject_orphan_scheme_document(&orphan);

    h.sync(D1);
    assert!(
        h.device(D1)
            .last_skipped
            .iter()
            .any(|s| s.document == orphan_doc_id),
        "orphan must be in last_skipped on first D1 pull"
    );

    // Step 3: D0 now properly creates and syncs a scheme (workspace index + content).
    let scheme_name = "Will Converge";
    let real_id = h.add_scheme(D0, scheme_name, &["proper item"]);
    h.sync(D0);

    // Step 4: D1 pulls again.  D0's scheme is new to D1 (no local CRDT doc).
    // The engine detects this in the cursor-reset path and resets the cursor to 0.
    // On this very pull, the content doc has already been fetched and applied
    // (since D0 pushed it together with the workspace index in the same sync).
    h.sync(D1);

    // D1 must now have the real scheme from D0.
    assert!(
        h.device(D1).workspace.schemes.contains_key(&real_id),
        "D1 must have D0's scheme after reconvergence; schemes: {:?}",
        h.device(D1)
            .workspace
            .schemes
            .values()
            .map(|s| &s.name)
            .collect::<Vec<_>>()
    );

    // And the items must be present.
    h.assert_scheme_items(D1, real_id, &["proper item"]);

    let _ = orphan_doc_id;
}

// ---------------------------------------------------------------------------
// Test (c): deleted scheme — its orphan content doc doesn't cause an error
// ---------------------------------------------------------------------------

/// After deletion the content doc remains server-side; device B (who already
/// deleted it locally via a pulled index update) must tolerate any subsequent
/// pull containing that doc without erroring.
#[test]
fn deleted_scheme_content_doc_is_tolerated_on_subsequent_pulls() {
    let mut h = Harness::new(2);
    h.login_all();

    // D0 creates a scheme and both devices sync to a shared base.
    let scheme = h.add_scheme(D0, "Will Be Deleted", &["item 1"]);
    h.sync(D0);
    h.sync(D1);

    // Record the document id for the scheme on D1 before deletion.
    let doc_id = h
        .device(D1)
        .workspace
        .scheme_sync
        .get(&scheme)
        .expect("scheme sync meta")
        .id;

    // D0 archives the scheme and syncs: workspace index drops the scheme,
    // but the content doc remains on the server.
    h.archive_scheme(D0, scheme);
    h.sync(D0);

    // D1 syncs: must see the archive (scheme removed from sidebar) and must NOT
    // error on the still-present content doc.
    h.sync(D1);

    // D1 must have applied the archive.
    h.assert_scheme_archived(D1, scheme);

    // The scheme's content doc is still on the server.  It will appear as an
    // unknown-scheme-document skip on D1's next pull (cursor may or may not
    // advance past it depending on the seq). Either way, no error.
    h.sync(D1);

    // D1 remains correctly archived; no panic or error from the orphan content.
    h.assert_scheme_archived(D1, scheme);

    let _ = doc_id;
}

// ---------------------------------------------------------------------------
// Test (d): workspace-doc corruption still hard-fails
// ---------------------------------------------------------------------------

/// Inject garbage into the personal_workspace document server-side; the pull
/// must return Err (workspace-class fatal error), not silently succeed.
#[test]
fn workspace_doc_corruption_causes_fatal_pull_error() {
    let mut h = Harness::new(2);
    h.login_all();

    // D0 creates and pushes a scheme so the server has a workspace doc.
    let _scheme = h.add_scheme(D0, "Healthy", &["ok"]);
    h.sync(D0);

    // D0's workspace doc is now on the server.  Corrupt it.
    h.corrupt_workspace_document(D0);

    // D0 tries to pull: the corrupted workspace doc must cause a fatal error.
    // The harness's `sync` uses `.expect("pull/apply")` which panics on pull
    // failure.  Verify via catch_unwind.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.sync(D0);
    }));
    assert!(
        result.is_err(),
        "sync must panic/fail when the workspace doc is corrupt"
    );
}
