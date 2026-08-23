//! Encoding a document's persisted state from a thread that does not own it.
use super::super::*;

use super::helpers::add_root_scheme;
use knotq_model::Item;

fn workspace_with_a_scheme() -> (Workspace, SchemeId) {
    let mut workspace = Workspace::new();
    let scheme_id = add_root_scheme(&mut workspace, "Plan");
    workspace
        .schemes
        .get_mut(&scheme_id)
        .expect("scheme")
        .items
        .push(Item::new("A line"));
    workspace.ensure_sync_metadata();
    (workspace, scheme_id)
}

/// A handle must produce exactly what asking the document directly produces.
/// If these ever diverged, a save would persist different bytes depending on
/// which thread happened to write it.
#[test]
fn a_handle_encodes_what_the_document_itself_would() {
    let (workspace, _) = workspace_with_a_scheme();
    let docs = WorkspaceCrdtDocuments::try_new(&workspace).expect("build");

    let direct = docs.document_states();
    let handles = docs.document_state_handles();

    assert_eq!(direct.len(), handles.len());
    for (document, bytes) in &direct {
        let handle = handles.get(document).expect("a handle per document");
        assert_eq!(
            handle.encode().as_ref(),
            bytes.as_ref(),
            "handle disagreed with the document for {document}"
        );
    }
}

/// The handle shares its document's cache, so an edit must invalidate it —
/// otherwise a save would keep writing the state from before the edit.
#[test]
fn a_handle_sees_an_edit_made_after_it_was_taken() {
    let (mut workspace, scheme_id) = workspace_with_a_scheme();
    let mut docs = WorkspaceCrdtDocuments::try_new(&workspace).expect("build");
    let handles = docs.document_state_handles();
    let before = handles
        .values()
        .map(|handle| handle.encode())
        .collect::<Vec<_>>();

    workspace
        .schemes
        .get_mut(&scheme_id)
        .expect("scheme")
        .items
        .push(Item::new("A second line"));
    let outcome = docs.sync_changes(
        &workspace,
        &WorkspaceCrdtChangeSet::default().touch_scheme(scheme_id),
    );
    assert!(outcome.is_ok(), "{:?}", outcome.errors);

    let after = handles
        .values()
        .map(|handle| handle.encode())
        .collect::<Vec<_>>();
    assert_ne!(before, after, "the handle served a stale cache after an edit");
    // And it still agrees with the document.
    let direct = docs.document_states();
    for (document, handle) in &handles {
        assert_eq!(handle.encode().as_ref(), direct[document].as_ref());
    }
}

/// The point of the handle: it can be moved to another thread and encoded
/// there, which is what keeps a large scheme's serialization off the UI thread.
#[test]
fn handles_can_encode_on_another_thread() {
    let (workspace, _) = workspace_with_a_scheme();
    let docs = WorkspaceCrdtDocuments::try_new(&workspace).expect("build");
    let direct = docs.document_states();
    let handles = docs.document_state_handles();

    let encoded = std::thread::spawn(move || {
        handles
            .into_iter()
            .map(|(document, handle)| (document, handle.encode()))
            .collect::<HashMap<_, _>>()
    })
    .join()
    .expect("the encoding thread");

    assert_eq!(encoded.len(), direct.len());
    for (document, bytes) in &direct {
        assert_eq!(encoded[document].as_ref(), bytes.as_ref());
    }
}
