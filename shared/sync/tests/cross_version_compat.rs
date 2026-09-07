//! Cross-version compatibility: an OLD build must not *die* just because it meets
//! newer data or a newer server.
//!
//! The bug on the old build may still be present — that is expected and is what
//! `AGENTS.md`'s "update every device build" guidance is for — but nothing here
//! is allowed to hard-fail, panic, wipe local data, or wedge every future sync.
//! The deliberate incompatibility break is the protocol-version floor
//! (`client_protocol_outdated`), which every client already surfaces as a clean
//! "please update" rather than a crash; this file pins the *graceful* handling
//! of everything short of that.

mod common;

use std::cell::RefCell;
use std::collections::HashMap;

use common::{TestDevice, TestServer};
use knotq_model::{DocumentId, SyncDocumentKind, Workspace, WorkspaceId};
use knotq_sync::{
    BatchPullRequest, BatchPullResponse, BatchPushRequest, BatchPushResponse, LocalSyncState,
    NotificationScheduleSnapshot, PersistedCrdtState, SyncPushRejected, SyncTransport,
    CLIENT_SYNC_PROTOCOL_VERSION,
};

fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    base.ensure_sync_metadata();
    TestDevice::new_from_base(&base, account)
}

// ---------------------------------------------------------------------------
// 1. A newer server response carrying fields this build has never seen must
//    still deserialize — serde ignores unknown fields, and every optional field
//    is `#[serde(default)]`, so a v2 response reads fine on v1.
// ---------------------------------------------------------------------------

#[test]
fn newer_pull_response_with_unknown_fields_still_parses() {
    let doc = DocumentId::new();
    let json = serde_json::json!({
        "documents": [{
            "document": doc.to_string(),
            "kind": "scheme",
            "seq": 4,
            "epoch": 0,
            "state_v1": "",
            // fields a future build might add to each document
            "compaction_generation": 7,
            "server_authored_at": "2027-01-01T00:00:00Z"
        }],
        "known_documents": { doc.to_string(): 4 },
        "has_more": false,
        // top-level fields a future build might add
        "presence_epoch": 12,
        "server_capabilities": ["comments", "presence-persist"]
    });

    let parsed: BatchPullResponse =
        serde_json::from_value(json).expect("a v2-shaped pull response must parse on this build");
    assert_eq!(parsed.documents.len(), 1);
    assert_eq!(parsed.documents[0].seq, 4);
    assert_eq!(parsed.documents[0].kind, SyncDocumentKind::Scheme);
    assert_eq!(parsed.known_documents.unwrap()[&doc], 4);
}

#[test]
fn newer_push_response_with_unknown_fields_still_parses() {
    let json = serde_json::json!({
        "documents": [{ "document": DocumentId::new().to_string(), "seq": 2, "accepted": 1,
                        "server_generation": 99 }],
        "notification_schedule_revision": 3,
        "future_flag": true
    });
    let parsed: BatchPushResponse =
        serde_json::from_value(json).expect("a v2-shaped push response must parse on this build");
    assert_eq!(parsed.documents.len(), 1);
    assert_eq!(parsed.notification_schedule_revision, 3);
}

// ---------------------------------------------------------------------------
// 2. Newer on-disk state must never read back as "there is no data". Both the
//    CRDT-state file and the local-sync-state file are consumed as
//    `unwrap_or_default()` by the drivers, so a parse error there re-seeds the
//    account from nothing (see AGENTS.md). Unknown fields must be ignored.
// ---------------------------------------------------------------------------

#[test]
fn newer_persisted_crdt_state_with_unknown_fields_keeps_its_documents() {
    let doc = DocumentId::new();
    let json = serde_json::json!({
        "documents": [{ "document": doc.to_string(), "state_v1": "", "epoch_hint": 4 }],
        "layout_version": 99,
        "written_by": "knotq/9.9.9"
    });
    let parsed: PersistedCrdtState =
        serde_json::from_value(json).expect("a v2 CRDT-state file must not read as empty");
    assert_eq!(parsed.documents.len(), 1, "the document must survive the parse");
    assert_eq!(parsed.documents[0].document, doc);
}

#[test]
fn newer_local_sync_state_with_unknown_fields_keeps_cursors_and_pending() {
    let doc = DocumentId::new();
    let json = serde_json::json!({
        "workspace_id": WorkspaceId::new().to_string(),
        "replica_id": knotq_model::ReplicaId::new().to_string(),
        "server_url": "https://api.knotq.com",
        "document_cursors": {
            doc.to_string(): {
                "document": doc.to_string(),
                "kind": "scheme",
                "last_pulled_sequence": 3,
                "last_pushed_sequence": 3,
                "epoch": 0,
                "future_per_cursor_field": 1
            }
        },
        "pending": [],
        // future top-level knobs
        "squash_policy": "aggressive",
        "presence_token": "abc"
    });
    let parsed: LocalSyncState =
        serde_json::from_value(json).expect("a v2 sync-state file must not read as empty");
    assert_eq!(parsed.document_cursors.len(), 1);
    assert_eq!(parsed.document_cursors[&doc].last_pulled_sequence, 3);
}

// ---------------------------------------------------------------------------
// 3. Every request carries `client_protocol_version`, so the server can gate an
//    incompatible old client with `client_protocol_outdated` instead of merging
//    wire-incompatible state into it.
// ---------------------------------------------------------------------------

struct CaptureTransport {
    pull_versions: RefCell<Vec<u32>>,
    push_versions: RefCell<Vec<u32>>,
}

impl SyncTransport for CaptureTransport {
    fn pull(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        self.pull_versions
            .borrow_mut()
            .push(request.client_protocol_version);
        Ok(BatchPullResponse {
            known_documents: Some(HashMap::new()),
            ..BatchPullResponse::default()
        })
    }
    fn push(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        self.push_versions
            .borrow_mut()
            .push(request.client_protocol_version);
        Ok(BatchPushResponse::default())
    }
}

#[test]
fn every_sync_request_states_its_protocol_version() {
    let account = WorkspaceId::new();
    let mut device = fresh_device(account);
    device.add_scheme("Plan", &["a"]);

    let transport = CaptureTransport {
        pull_versions: RefCell::new(Vec::new()),
        push_versions: RefCell::new(Vec::new()),
    };
    // A full sync cycle over the capturing transport (drives the real engine).
    let _ = device.try_sync_with(&transport);

    assert!(
        transport
            .pull_versions
            .borrow()
            .iter()
            .all(|v| *v == CLIENT_SYNC_PROTOCOL_VERSION),
        "pull must always send this build's protocol version"
    );
    assert!(
        transport
            .push_versions
            .borrow()
            .iter()
            .all(|v| *v == CLIENT_SYNC_PROTOCOL_VERSION),
        "push must always send this build's protocol version"
    );
}

// ---------------------------------------------------------------------------
// 4. A `client_protocol_outdated` rejection is a clean, bounded, typed error —
//    never a panic and never an infinite self-heal loop.
// ---------------------------------------------------------------------------

struct AlwaysRejectPush {
    code: String,
}

impl SyncTransport for AlwaysRejectPush {
    fn pull(&self, _request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        Ok(BatchPullResponse {
            known_documents: Some(HashMap::new()),
            ..BatchPullResponse::default()
        })
    }
    fn push(&self, _request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        Err(anyhow::Error::new(SyncPushRejected {
            code: self.code.clone(),
        }))
    }
}

#[test]
fn protocol_outdated_push_rejection_is_bounded_not_a_wedge() {
    let account = WorkspaceId::new();
    let mut device = fresh_device(account);
    device.add_scheme("Plan", &["will not push"]);

    let transport = AlwaysRejectPush {
        code: "client_protocol_outdated".to_string(),
    };
    // Pull succeeds; the push is rejected. This must return an `Err` in bounded
    // time (one self-heal reseed attempt at most), not loop or panic.
    let result = device.try_sync_with(&transport);
    assert!(
        result.is_err(),
        "a persistent push rejection must surface as an error"
    );
    // The device's local content is untouched — nothing was lost by the failed sync.
    assert_eq!(
        device
            .workspace
            .schemes
            .values()
            .flat_map(|s| s.items.iter().map(|i| i.text()))
            .collect::<Vec<_>>(),
        vec!["will not push".to_string()]
    );
}

// ---------------------------------------------------------------------------
// 5. A pulled document whose epoch is *ahead* of anything this build recorded
//    (a newer squash mechanism) is adopted or skipped — never a panic.
// ---------------------------------------------------------------------------

/// Replays the current merged state of the wrapped `TestServer`, but overrides
/// one document's `epoch` with an absurd future value on the first pull — as if
/// a newer squash mechanism had bumped it far past anything this build mints.
struct FutureEpochPull<'a> {
    inner: &'a TestServer,
    document: DocumentId,
    served: RefCell<bool>,
}

impl SyncTransport for FutureEpochPull<'_> {
    fn pull(&self, request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        let mut response = self.inner.pull(request)?;
        if !*self.served.borrow() {
            *self.served.borrow_mut() = true;
            for doc in &mut response.documents {
                if doc.document == self.document {
                    doc.epoch = 999_999;
                    doc.seq = doc.seq.max(1);
                }
            }
            if let Some(known) = &mut response.known_documents {
                if let Some(seq) = known.get_mut(&self.document) {
                    *seq = (*seq).max(1);
                }
            }
        }
        Ok(response)
    }
    fn push(&self, request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        self.inner.push(request)
    }
}

#[test]
fn a_document_epoch_from_the_future_does_not_panic() {
    // A device that has synced a scheme at epoch 0 later meets the same scheme
    // document stamped with an epoch from a future build. It must adopt or skip
    // it — never panic, never wedge every later sync.
    let account = WorkspaceId::new();
    let server = TestServer::default();

    let mut producer = fresh_device(account);
    let scheme = producer.add_scheme("Squashed elsewhere", &["line one", "line two"]);
    producer.try_sync(&server).expect("seed the server");
    let document = producer.workspace.scheme_sync[&scheme].id;

    let mut device = fresh_device(account);
    device.try_sync(&server).expect("device learns the scheme at epoch 0");

    let future = FutureEpochPull {
        inner: &server,
        document,
        served: RefCell::new(false),
    };
    // Produce a new merged state on the server so the doc is actually re-served.
    producer.append_line(scheme, "line three");
    producer.try_sync(&server).expect("bump the doc");

    // The future-epoch pull must return without panicking.
    let _ = device.try_sync_with(&future);
    // And an ordinary follow-up sync must still work.
    device
        .try_sync(&server)
        .expect("device stays usable after meeting a future epoch");
    assert!(
        device.workspace.schemes.contains_key(&scheme),
        "the scheme is still present after the future-epoch encounter"
    );
}

// Silence unused-import lints on some feature combinations.
#[allow(dead_code)]
fn _keep(_: NotificationScheduleSnapshot, _: fn(&dyn SyncTransport), _: BatchPushRequest) {}
