//! Integration tests for the KnotQ sync engine against the REAL Cloudflare
//! Worker backend running in local dev mode via `wrangler dev`.
//!
//! ## How to run
//!
//! Use the orchestration script (from the workspace root):
//!
//! ```sh
//! ./local/run-sync-integration.sh
//! ```
//!
//! Or manually:
//!
//! ```sh
//! # Terminal 1 — start the backend
//! cd app/backend/cloudflare
//! pnpm wrangler dev --port 8788 --var KNOTQ_TEST_MODE:1 \
//!   --persist-to .wrangler/integration-test-state
//!
//! # Terminal 2 — run the tests
//! export KNOTQ_SYNC_BACKEND_URL=http://127.0.0.1:8788
//! cargo test -p knotq-sync --test backend_integration -- --nocapture
//! ```
//!
//! If `KNOTQ_SYNC_BACKEND_URL` is unset the tests skip with a notice.
//!
//! ## Isolation
//!
//! Every test calls `/__test/bootstrap` with a UUID-keyed email so each test
//! gets a fresh isolated backend workspace. Tests can run in parallel safely.

mod common;

use std::collections::HashMap;
use std::env;

use chrono::Utc;
use common::http_transport::{
    backend_bootstrap, orphan_push_request, unique_test_email, HttpClient,
};
use common::TestDevice;
use knotq_model::{
    DocumentId, Item, ReplicaId, Scheme, SchemeId, SyncDocumentKind, Workspace, WorkspaceId,
};
use knotq_sync::{
    BatchPullRequest, BatchPushRequest, NotificationScheduleSnapshot, PushDocumentUpdates,
    SyncTransport, WorkspaceCrdtDocuments,
};

// ---------------------------------------------------------------------------
// Helper — skip when the backend URL is not configured
// ---------------------------------------------------------------------------

fn backend_url() -> Option<String> {
    match env::var("KNOTQ_SYNC_BACKEND_URL") {
        Ok(url) if !url.is_empty() => Some(url.trim_end_matches('/').to_string()),
        _ => {
            println!(
                "[backend_integration] KNOTQ_SYNC_BACKEND_URL not set — skipping. \
                 Run ./local/run-sync-integration.sh to exercise real-backend scenarios."
            );
            None
        }
    }
}

/// Build a workspace and `TestDevice` using the backend's provisioned workspace_id.
fn make_device(workspace_id: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(workspace_id);
    base.ensure_sync_metadata();
    TestDevice::new_from_base(&base, workspace_id)
}

fn dummy_schedule() -> NotificationScheduleSnapshot {
    let now = Utc::now();
    NotificationScheduleSnapshot {
        sequence: 0,
        // The real backend requires a 64-char sha256 hex hash and a non-empty
        // window (window_end > window_start).
        hash: "0".repeat(64),
        window_start: now,
        window_end: now + chrono::Duration::hours(1),
        occurrence_count: 0,
    }
}

// ---------------------------------------------------------------------------
// Scenario a — Two-device convergence
// ---------------------------------------------------------------------------

#[test]
fn two_device_convergence() {
    let Some(base_url) = backend_url() else {
        return;
    };

    // Bootstrap: two calls to /__test/bootstrap with the SAME email produce
    // the same user_id/workspace_id but fresh bearer tokens.
    let email = unique_test_email("converge");
    let resp_a = backend_bootstrap(&base_url, &email).expect("bootstrap A");
    let resp_b = backend_bootstrap(&base_url, &email).expect("bootstrap B");
    let workspace_id: WorkspaceId = resp_a.workspace_id.parse().expect("uuid");

    let mut device_a = make_device(workspace_id);
    let mut device_b = make_device(workspace_id);

    let client_a = HttpClient::from_bootstrap(&base_url, &resp_a);
    let client_b = HttpClient::from_bootstrap(&base_url, &resp_b);

    // Device A creates a scheme with two items.
    let scheme = device_a.add_scheme("Shared Plan", &["alpha", "beta"]);
    device_a.try_sync_with(&client_a).expect("A push");

    // Device B pulls and sees the scheme.
    device_b.try_sync_with(&client_b).expect("B pull");
    assert!(
        device_b.workspace.schemes.contains_key(&scheme),
        "Device B must discover the scheme after pull"
    );
    let items_b: Vec<String> = device_b.workspace.schemes[&scheme]
        .items
        .iter()
        .map(|item| item.text.clone())
        .collect();
    assert!(
        items_b.iter().any(|t| t == "alpha") && items_b.iter().any(|t| t == "beta"),
        "Device B must see both items; got: {items_b:?}"
    );

    // Device B appends a line and pushes.
    device_b.append_line(scheme, "gamma");
    device_b.try_sync_with(&client_b).expect("B push");

    // Device A pulls and should see the gamma line.
    device_a.try_sync_with(&client_a).expect("A pull2");
    let items_a: Vec<String> = device_a.workspace.schemes[&scheme]
        .items
        .iter()
        .map(|i| i.text.clone())
        .collect();
    assert!(
        items_a.iter().any(|t| t == "gamma"),
        "Device A must see gamma after convergence; items: {items_a:?}"
    );

    // Convergence check: both device summaries should match now.
    // (We check the scheme items are identical — same set regardless of order.)
    let mut items_a_sorted = items_a.clone();
    let mut items_b2: Vec<String> = device_b.workspace.schemes[&scheme]
        .items
        .iter()
        .map(|i| i.text.clone())
        .collect();
    items_a_sorted.sort();
    items_b2.sort();
    assert_eq!(
        items_a_sorted, items_b2,
        "Both devices must converge to the same item set"
    );
}

// ---------------------------------------------------------------------------
// Scenario b — Media round-trip
// ---------------------------------------------------------------------------

#[test]
fn media_round_trip() {
    let Some(base_url) = backend_url() else {
        return;
    };

    let email = unique_test_email("media");
    let resp_a = backend_bootstrap(&base_url, &email).expect("bootstrap A");
    let workspace_id: WorkspaceId = resp_a.workspace_id.parse().expect("uuid");

    let mut device_a = make_device(workspace_id);
    let client_a = HttpClient::from_bootstrap(&base_url, &resp_a);

    // Create a scheme and push so the server knows the document.
    let scheme = device_a.add_scheme("Media Test", &["image item"]);
    device_a.try_sync_with(&client_a).expect("A push");

    // Attach ~100 KB image.
    let image_bytes: Vec<u8> = (0u32..102_400).map(|i| (i % 251) as u8).collect();
    let (_, image_name) = device_a.attach_image(scheme, 0, image_bytes.clone());

    // Sync CRDT (registers the media reference).
    device_a
        .try_sync_with(&client_a)
        .expect("A push media crdt");

    // Find the scheme's document id for the media URL.
    let scheme_doc_id = device_a
        .workspace
        .scheme_sync
        .get(&scheme)
        .expect("scheme sync meta")
        .id;

    // Upload the image.
    client_a
        .upload_media(scheme_doc_id, &image_name, &image_bytes)
        .expect("upload media");

    // --- Device B ---
    let resp_b = backend_bootstrap(&base_url, &email).expect("bootstrap B");
    let workspace_id_b: WorkspaceId = resp_b.workspace_id.parse().expect("uuid");
    assert_eq!(
        workspace_id, workspace_id_b,
        "both bootstraps for same email must share workspace_id"
    );
    let mut device_b = make_device(workspace_id);
    let client_b = HttpClient::from_bootstrap(&base_url, &resp_b);

    // B pulls CRDT (discovers the scheme and media reference).
    device_b.try_sync_with(&client_b).expect("B pull");

    // B downloads the image.
    let scheme_doc_b = device_b
        .workspace
        .scheme_sync
        .get(&scheme)
        .map(|meta| meta.id);
    let Some(doc_b) = scheme_doc_b else {
        panic!("Device B did not discover the scheme after sync");
    };
    let downloaded = client_b
        .download_media(doc_b, &image_name)
        .expect("download media")
        .expect("image must be present on server");

    assert_eq!(
        downloaded, image_bytes,
        "downloaded bytes must be byte-identical to uploaded bytes"
    );

    // --- Verify >3 MiB upload is rejected (client-side check). ---
    let oversized = vec![0xABu8; knotq_sync::MAX_SYNC_MEDIA_BYTES + 1];
    let oversized_result = client_a.upload_media(scheme_doc_id, "oversized.png", &oversized);
    assert!(
        oversized_result.is_err(),
        "upload exceeding MAX_SYNC_MEDIA_BYTES must fail (client-side)"
    );
}

// ---------------------------------------------------------------------------
// Scenario c — Orphan tolerance
// ---------------------------------------------------------------------------

#[test]
fn orphan_tolerance() {
    let Some(base_url) = backend_url() else {
        return;
    };

    let email = unique_test_email("orphan");
    let resp = backend_bootstrap(&base_url, &email).expect("bootstrap");
    let workspace_id: WorkspaceId = resp.workspace_id.parse().expect("uuid");
    let mut device = make_device(workspace_id);
    let client = HttpClient::from_bootstrap(&base_url, &resp);

    // Push a legitimate scheme so the server has some content.
    let scheme = device.add_scheme("Real Scheme", &["real item"]);
    device.try_sync_with(&client).expect("push real scheme");

    // Build a valid scheme CRDT update for an orphan document (no workspace-index entry).
    let orphan_doc = DocumentId::new();
    let orphan_update = build_valid_scheme_update(orphan_doc);
    // Push it directly — bypasses the engine so there is no workspace-index entry.
    let orphan_req = orphan_push_request(orphan_doc, SyncDocumentKind::Scheme, orphan_update);
    client
        .push(&orphan_req)
        .expect("orphan push must succeed on server");

    // Reset the pull cursor so the device re-fetches everything.
    device.local_state_mut().document_cursors.clear();

    // Pull — must NOT fail even with the orphan.
    device
        .try_sync_with(&client)
        .expect("sync after orphan injection must succeed");

    // Real scheme must still be present.
    assert!(
        device.workspace.schemes.contains_key(&scheme),
        "real scheme must survive pull with orphan"
    );

    // Orphan must appear in skipped with unknown_scheme_document=true.
    let skipped = &device.last_skipped;
    let orphan_skip = skipped.iter().find(|s| s.document == orphan_doc);
    assert!(
        orphan_skip.is_some(),
        "orphan must appear in PullOutcome.skipped; skipped: {skipped:?}"
    );
    assert!(
        orphan_skip.unwrap().unknown_scheme_document,
        "orphan skip must have unknown_scheme_document=true"
    );
}

/// Build a minimal valid Yjs scheme update suitable for server-side injection.
/// Mirrors the TypeScript `buildSchemeUpdate` in test/crdt-helpers.ts.
fn build_valid_scheme_update(doc_id: DocumentId) -> Vec<u8> {
    let scheme_id = SchemeId::new();
    let mut workspace = Workspace::new();
    workspace.ensure_sync_metadata();
    let mut scheme = Scheme::new("Orphan", 0);
    scheme.id = scheme_id;
    scheme.items.push(Item::new("orphan item"));
    workspace.schemes.insert(scheme_id, scheme);
    workspace.ensure_sync_metadata();

    // Override the scheme's sync doc id to the target doc_id.
    if let Some(meta) = workspace.scheme_sync.get_mut(&scheme_id) {
        meta.id = doc_id;
    }

    WorkspaceCrdtDocuments::snapshot_updates(&workspace)
        .updates
        .into_iter()
        .find(|u| u.document == doc_id)
        .map(|u| u.update_v1)
        .expect("scheme update in snapshot")
}

// ---------------------------------------------------------------------------
// Scenario d — Restart with pending edits (local_sequence wedge regression)
// ---------------------------------------------------------------------------

#[test]
fn restart_with_pending_edits() {
    let Some(base_url) = backend_url() else {
        return;
    };

    let email = unique_test_email("restart");
    let resp = backend_bootstrap(&base_url, &email).expect("bootstrap");
    let workspace_id: WorkspaceId = resp.workspace_id.parse().expect("uuid");
    let mut device = make_device(workspace_id);
    let client = HttpClient::from_bootstrap(&base_url, &resp);

    // Push an initial snapshot.
    let scheme = device.add_scheme("Restart Test", &["seed"]);
    device.try_sync_with(&client).expect("initial push");

    // Make more edits than PUSH_MAX_UPDATES_PER_DOCUMENT to stress the multi-batch path.
    use knotq_sync::PUSH_MAX_UPDATES_PER_DOCUMENT;
    for i in 0..PUSH_MAX_UPDATES_PER_DOCUMENT + 5 {
        device.append_line(scheme, &format!("offline-{i}"));
    }

    // Restart with the FIXED sequence seeding (no duplicate sequences).
    device.restart();

    // Push — must succeed with no crdt_schema_invalid.
    device
        .try_sync_with(&client)
        .expect("push after restart must not produce crdt_schema_invalid");

    // Verify: a fresh device sees all offline edits.
    let resp2 = backend_bootstrap(&base_url, &email).expect("bootstrap2");
    let mut device2 = make_device(workspace_id);
    let client2 = HttpClient::from_bootstrap(&base_url, &resp2);
    device2.try_sync_with(&client2).expect("device2 pull");

    let expected = format!("offline-{}", PUSH_MAX_UPDATES_PER_DOCUMENT + 4);
    let items: Vec<String> = device2
        .workspace
        .schemes
        .get(&scheme)
        .map(|s| s.items.iter().map(|i| i.text.clone()).collect())
        .unwrap_or_default();
    assert!(
        items.iter().any(|t| t == &expected),
        "last offline line must reach the server after restart+push; items: {items:?}"
    );
}

// ---------------------------------------------------------------------------
// Scenario e — Atomic batch rejection + verification
// ---------------------------------------------------------------------------

#[test]
fn atomic_batch_rejection_and_server_gate() {
    let Some(base_url) = backend_url() else {
        return;
    };

    let email = unique_test_email("atomic");
    let resp = backend_bootstrap(&base_url, &email).expect("bootstrap");
    let client = HttpClient::from_bootstrap(&base_url, &resp);

    let doc_id = DocumentId::new();
    let schedule = dummy_schedule();

    // Push a garbage (invalid Yjs) update — server must reject.
    let garbage_request = BatchPushRequest {
        replica_id: ReplicaId::new(),
        documents: vec![PushDocumentUpdates {
            document: doc_id,
            kind: SyncDocumentKind::Scheme,
            // Not a valid Yjs v1 update — just random bytes.
            updates: vec![vec![1u8, 2, 3, 4, 5, 6, 7, 8]],
        }],
        notification_schedule_changed: false,
        notification_schedule: Some(schedule.clone()),
    };
    let rejection = client.push(&garbage_request);
    assert!(
        rejection.is_err(),
        "garbage Yjs update must be rejected by the server"
    );
    let err_str = rejection.unwrap_err().to_string();
    // The server returns crdt_schema_invalid or update_payload_invalid (both signal
    // the same "bad bytes" case — the exact code depends on where validation fails).
    assert!(
        err_str.contains("crdt_schema_invalid")
            || err_str.contains("update_payload_invalid")
            || err_str.contains("rejected"),
        "rejection must carry a machine-readable error code; got: {err_str}"
    );

    // The document must NOT be persisted (atomic batch rejection).
    let pull = client
        .pull(&BatchPullRequest {
            replica_id: ReplicaId::new(),
            cursors: HashMap::new(),
        })
        .expect("pull after rejection");
    let persisted = pull.documents.iter().any(|d| d.document == doc_id);
    assert!(
        !persisted,
        "rejected document must NOT appear in pull after atomic batch rejection"
    );

    // A valid push for the same document must succeed (server state is clean).
    let valid_update = build_valid_scheme_update(doc_id);
    let valid_request = orphan_push_request(doc_id, SyncDocumentKind::Scheme, valid_update);
    let accepted = client
        .push(&valid_request)
        .expect("valid push must succeed");
    assert_eq!(accepted.documents.len(), 1);
    assert_eq!(accepted.documents[0].document, doc_id);
    assert_eq!(accepted.documents[0].accepted, 1);

    // Engine self-heal path (reseed) — note: organically triggering crdt_schema_invalid
    // from a well-formed TestDevice is not practical here (the engine validates before
    // pushing). The self-heal path is covered in the in-memory regression suite
    // (sync_wedge_regressions.rs: `schema_invalid_rejection_must_self_heal`). What we
    // verify here is the raw server-side atomicity contract that the self-heal relies on:
    // a rejected batch leaves the server state untouched so the reseeded push can succeed.
    println!("[atomic_batch_rejection] verified: server atomically rejects bad batches and accepts clean reseed");
}

// ---------------------------------------------------------------------------
// Hard scenario helpers
// ---------------------------------------------------------------------------

/// Bootstrap `n` devices sharing one workspace (same email, separate bearer tokens).
fn bootstrap_harness(base_url: &str, label: &str, n: usize) -> common::Harness {
    let email = common::http_transport::unique_test_email(label);
    let mut tokens = Vec::new();
    let mut workspace_id_str = String::new();
    for i in 0..n {
        let resp =
            backend_bootstrap(base_url, &email).unwrap_or_else(|e| panic!("bootstrap {i}: {e}"));
        if workspace_id_str.is_empty() {
            workspace_id_str = resp.workspace_id.clone();
        }
        tokens.push(resp.bearer_token);
    }
    let workspace_id: WorkspaceId = workspace_id_str.parse().expect("workspace_id uuid");
    common::Harness::new_http(base_url, workspace_id, tokens)
}

// ---------------------------------------------------------------------------
// Scenario a — Edit-vs-delete (HTTP)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_a_edit_vs_delete_a_first() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "a-edit-del-a", 2);
    common::scenarios::scenario_a_edit_vs_delete_a_first(&mut h);
}

#[test]
fn http_scenario_a_edit_vs_delete_b_first() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "a-edit-del-b", 2);
    common::scenarios::scenario_a_edit_vs_delete_b_first(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario b — Delete-vs-archive race (HTTP)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_b_delete_vs_archive_race() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "b-del-arch", 2);
    common::scenarios::scenario_b_delete_vs_archive_race(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario c — Folder shuffle storm (HTTP)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_c_folder_shuffle_storm() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "c-folder-storm", 2);
    common::scenarios::scenario_c_folder_shuffle_storm(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario d — Zig-zag interleave (HTTP)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_d_zigzag_interleave() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "d-zigzag", 2);
    common::scenarios::scenario_d_zigzag_interleave(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario e — Long offline divergence (HTTP, bounded ops)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_e_long_offline_divergence() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "e-offline", 2);
    // Uses the same scenario but it will run with PUSH_MAX_UPDATES_PER_DOCUMENT*2+10 ops.
    common::scenarios::scenario_e_long_offline_divergence(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario f — Offline + restart combo (HTTP)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_f_offline_restart_combo() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "f-restart", 2);
    common::scenarios::scenario_f_offline_restart_combo(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario g — Daily queue conflicts (HTTP)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_g_daily_queue_conflicts() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "g-daily", 2);
    common::scenarios::scenario_g_daily_queue_conflicts(&mut h);
}

#[test]
fn http_scenario_g2_daily_queue_direct_creation() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "g2-daily-direct", 2);
    common::scenarios::scenario_g2_daily_queue_direct_creation(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario h — Calendar import lifecycle (HTTP)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_h_calendar_lifecycle() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "h-cal", 2);
    common::scenarios::scenario_h_calendar_import_lifecycle(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario i — Media variants (HTTP, skips the oversized check since that's
// client-side and already covered in the existing media_round_trip test)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_i_media_concurrent_attach() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "i-media-attach", 2);
    common::scenarios::scenario_i_media_variants(&mut h);
}

#[test]
fn http_scenario_i_media_scheme_deleted() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "i-media-del", 2);
    common::scenarios::scenario_i_media_scheme_deleted(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario j — Notification schedule + doc edits (HTTP)
// The real backend returns monotonically increasing revision; assert that.
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_j_notification_schedule_monotonic() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "j-notif", 2);

    use common::{D0, D1};
    h.login_all();
    let s = h.add_scheme(D0, "Notify", &["task"]);
    h.settle();

    h.append_line(D0, s, "new task");
    let hash_a = "a".repeat(64);
    let rev_a = h.update_notification_schedule(D0, 1, &hash_a);

    h.append_line(D1, s, "D1 task");
    let hash_b = "b".repeat(64);
    let rev_b = h.update_notification_schedule(D1, 2, &hash_b);

    // Real backend: revision is monotonically non-decreasing.
    assert!(
        rev_b >= rev_a,
        "notification_schedule_revision must be monotonic: rev_a={rev_a} rev_b={rev_b}"
    );

    h.settle();
    h.assert_all_converged();
}

// ---------------------------------------------------------------------------
// Scenario k — Fresh device join (HTTP, 3 devices)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_k_fresh_device_join() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_harness(&base_url, "k-join", 3);
    common::scenarios::scenario_k_fresh_device_join(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario l — Seeded randomized fuzz (HTTP, reduced op count for speed)
// ---------------------------------------------------------------------------

#[test]
fn http_scenario_l_randomized_fuzz() {
    let Some(base_url) = backend_url() else {
        return;
    };
    // Run two fixed seeds over HTTP with 60 ops each.
    for seed in [42u64, 137] {
        let mut h = bootstrap_harness(&base_url, &format!("l-fuzz-{seed}"), 3);
        common::scenarios::scenario_l_randomized_fuzz(&mut h, seed, 60);
    }
}
