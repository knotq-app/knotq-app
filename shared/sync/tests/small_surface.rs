//! The small, pure corners of the sync crate: access roles, user ids, and the
//! document list a sync derives from a workspace.
//!
//! Each is a handful of lines with no I/O, and each is load-bearing — the role
//! decides whether a request may write at all, and `sync_documents` is the list
//! every pull and push iterates. They had no coverage, which is how a one-line
//! change to any of them would have gone unnoticed.

use knotq_model::{Scheme, SyncDocumentKind, Workspace};
use knotq_sync::{scheme_documents, sync_documents, AccessRole, UserId};

#[test]
fn only_an_owner_or_writer_may_write_and_every_role_may_read() {
    assert!(AccessRole::Owner.can_write());
    assert!(AccessRole::Writer.can_write());
    assert!(
        !AccessRole::Reader.can_write(),
        "a reader must never be able to push"
    );

    for role in [AccessRole::Owner, AccessRole::Writer, AccessRole::Reader] {
        assert!(role.can_read(), "{role:?} should be able to read");
    }
}

#[test]
fn user_ids_are_unique_and_round_trip_through_their_text_form() {
    let id = UserId::new();
    assert_ne!(id, UserId::new(), "two fresh ids must differ");
    assert_ne!(
        UserId::default(),
        UserId::default(),
        "the default is a fresh id, not a fixed one"
    );

    let parsed: UserId = id.to_string().parse().expect("a printed id parses back");
    assert_eq!(parsed, id);
    assert!(
        "not-a-uuid".parse::<UserId>().is_err(),
        "garbage must not parse into a user id"
    );
}

#[test]
fn a_workspace_syncs_its_index_document_plus_one_per_scheme() {
    let mut workspace = Workspace::new();
    let first = Scheme::new("Notes", 0);
    let second = Scheme::new("Work", 1);
    let (first_id, second_id) = (first.id, second.id);
    workspace.schemes.insert(first_id, first);
    workspace.schemes.insert(second_id, second);
    workspace.ensure_sync_metadata();

    let schemes = scheme_documents(&workspace);
    assert_eq!(schemes.len(), 2, "one document per scheme");
    assert!(schemes
        .iter()
        .all(|doc| doc.kind == SyncDocumentKind::Scheme));

    let all = sync_documents(&workspace);
    assert_eq!(
        all.len(),
        schemes.len() + 1,
        "the workspace index is synced alongside the schemes"
    );
    assert_eq!(all[0].document, workspace.sync.id);
    assert_eq!(all[0].kind, SyncDocumentKind::PersonalWorkspace);
}
