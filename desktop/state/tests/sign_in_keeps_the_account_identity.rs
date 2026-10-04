//! One property: **landing the first sync after signing in leaves this device on
//! the ACCOUNT's workspace identity, not its own pre-sign-in one.**
//!
//! A device that has not signed in yet has a local `WorkspaceId` and a local
//! workspace-index `DocumentId`, and `Workspace::new` draws the two
//! independently. Signing in canonicalizes both onto the account:
//! `canonicalize_personal_sync_identity_with_change` sets `id` to the account's
//! workspace id and *derives* `sync.id` from it.
//!
//! The first sync's result cannot be merged into the live documents (the run
//! re-identified the index document), so it lands through
//! `replace_from_sync`, which adopts the run's documents and then replays every
//! still-unpushed local edit on top. One of those replayed edits can be an index
//! write authored before sign-in — and an index write carries `meta.id` and
//! `meta.sync`. Yjs resolves a map key by last writer, so replaying it puts the
//! PRE-SIGN-IN identity back into the account's document, and the workspace
//! materialized from it carries that identity.
//!
//! **This test currently FAILS and is `#[ignore]`d: it pins an OPEN GAP, not a
//! regression.** The obvious fix — pass the account's canonical id, captured
//! before the replay — makes `reroot_pre_sign_in_edits` re-key `id`/`sync.id`,
//! which queues index edits after `remap_pending_workspace_document` has already
//! run, so chaos seed 178 ends wedged with five unpushed `PersonalWorkspace`
//! edits addressed to the stray document. A correct fix has to move the
//! document's content and the edits addressed to it, the way
//! `adopt_sync_workspace_identity` does on the merge path. Measured 2026-10-03.
//!
//! The data loss this gap used to cause is separately fixed and separately
//! pinned: see `carries_content` in `sync_service/snapshot.rs`, which stops the
//! re-identification rescue replacing a real index with an empty document.
//!
//! Re-rooting to that identity is the failure this pins. It does not merely
//! mislabel the workspace: `sync.id = DocumentId(workspace_id.0)` means the
//! device starts naming an index document **derived from a local WorkspaceId** —
//! an id no other device and no server has ever seen — while the account's real
//! index stays on disk under the canonical id. Every relaunch then builds the
//! index document EMPTY, `workspace_is_seeded()` is false,
//! `queue_local_only_documents_before_pull` declines to publish anything this
//! device holds, and the next pull materializes the account's index over its
//! local-only pages. Journal-loss fuzz seed 20223 loses a Daily page, its line
//! and its queue binding exactly this way.

use std::collections::HashMap;

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{DocumentId, NodeRef, ReplicaId, Scheme, Workspace, WorkspaceId};
use knotq_state::WorkspaceStore;
use knotq_sync::WorkspaceCrdtDocuments;

/// A never-signed-in install: one scheme, its own local identity, and an
/// UNPOPULATED index document — which is what a fresh install has, and what
/// makes its first index write a full population carrying `meta.id`/`meta.sync`
/// rather than a delta that carries neither.
fn pre_sign_in_store() -> (WorkspaceStore, Workspace) {
    let mut workspace = Workspace::new();
    let scheme = Scheme::new("Local notes", 0);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme.id));
    workspace.schemes.insert(scheme.id, scheme);
    workspace.ensure_sync_metadata();
    let store = WorkspaceStore::new(
        workspace.clone(),
        ReplicaId::new(),
        false,
        HashMap::<DocumentId, Vec<u8>>::new(),
        1,
    );
    (store, workspace)
}

/// What the sync run hands back: the same content under the account's identity.
fn canonical_result(pre_sign_in: &Workspace, account: WorkspaceId) -> (Workspace, Workspace) {
    let mut canonical = pre_sign_in.clone();
    canonical.canonicalize_personal_sync_identity_with_change(account);
    canonical.ensure_sync_metadata();
    (canonical.clone(), canonical)
}

#[test]
#[ignore = "open gap: fixing it by passing the canonical id wedges chaos 178 — see \
            WorkspaceStore::reroot_pre_sign_in_edits"]
fn landing_the_first_sync_keeps_the_accounts_workspace_identity() {
    let (mut store, pre_sign_in) = pre_sign_in_store();
    let account = WorkspaceId::new();
    assert_ne!(
        account, pre_sign_in.id,
        "the account's id must differ from the pre-sign-in one for this to test anything"
    );

    // An unpushed index edit authored BEFORE signing in. This is what gets
    // replayed over the account's document by `replace_from_sync`.
    store
        .apply_local(
            Command::CreateScheme {
                folder: store.workspace().root,
                name: "made before signing in".to_string(),
                color_index: 0,
                position: None,
            },
            CommandOrigin::User,
        )
        .unwrap();
    assert!(
        !store.pending_operations().is_empty(),
        "the pre-sign-in edit must still be queued, or nothing is replayed"
    );

    let (canonical, _) = canonical_result(&pre_sign_in, account);
    let states: HashMap<DocumentId, Vec<u8>> = WorkspaceCrdtDocuments::try_new(&canonical)
        .unwrap()
        .document_states()
        .into_iter()
        .map(|(document, state)| (document, state.to_vec()))
        .collect();

    store.replace_from_sync(canonical, states);

    assert_eq!(
        store.workspace().id,
        account,
        "landing the first sync left the store on its pre-sign-in WorkspaceId; every \
         identity derived from it follows, starting with the index document"
    );
    assert_eq!(
        store.workspace().sync.id,
        DocumentId(account.0),
        "the store is naming an index document derived from a local WorkspaceId. \
         Nothing else has that document: the account's index stays on disk under \
         the canonical id, so the next launch builds this one EMPTY and the pull \
         materializes the account's index over every local-only page"
    );
}
