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
    assert_ne!(
        before, after,
        "the handle served a stale cache after an edit"
    );
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

// ---------------------------------------------------------------------------
// Concurrency: the handle shares its document's cache with the thread that owns
// the document, so the two can be inside `get_shared` at the same time. Both of
// the interleavings below lose an edit, and the loss is silent: whoever reads
// the cache next gets bytes from before the edit and rebuilds the CRDT document
// from them.
// ---------------------------------------------------------------------------

use std::sync::mpsc;

/// A background encode in flight must not make a concurrent reader serve the
/// state from *before* the edit that is still being encoded.
///
/// The save task takes handles on the UI thread and encodes them on the
/// background executor. While that encode runs, anything on the UI thread that
/// asks for the document states — `sync_store_from_workspace` rebuilds the CRDT
/// documents from exactly those bytes — must not be handed the pre-edit state.
#[test]
fn a_reader_never_sees_the_pre_edit_state_while_a_background_encode_is_running() {
    let cache = Arc::new(EncodeCacheState::default());
    // Steady state: the document has been encoded once and nothing has changed.
    assert_eq!(cache.get_shared(|| vec![0]).as_ref(), [0]);

    // The user's edit lands.
    cache.mark_dirty();

    // The background save starts encoding it, and is still inside `encode`.
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let background = {
        let cache = Arc::clone(&cache);
        std::thread::spawn(move || {
            cache.get_shared(|| {
                entered_tx.send(()).expect("hand over to the reader");
                release_rx.recv().expect("wait for the reader");
                vec![1]
            })
        })
    };
    entered_rx.recv().expect("the background encode started");

    // Meanwhile the UI thread asks for the states.
    let read = cache.get_shared(|| vec![1]);
    release_tx.send(()).expect("release the background encode");
    background.join().expect("the background encode");

    assert_eq!(
        read.as_ref(),
        [1],
        "a reader was handed the state from before the edit because a background \
         encode had already consumed the dirty flag"
    );
}

/// A slow background encode must not publish its (older) result over a newer
/// one, leaving the cache serving pre-edit bytes to every later reader.
#[test]
fn a_slow_background_encode_never_overwrites_a_newer_state() {
    let cache = Arc::new(EncodeCacheState::default());
    cache.mark_dirty();

    // The background save is inside `encode`, holding the pre-edit content.
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let background = {
        let cache = Arc::clone(&cache);
        std::thread::spawn(move || {
            cache.get_shared(|| {
                entered_tx.send(()).expect("hand over to the reader");
                release_rx.recv().expect("wait for the reader");
                vec![0]
            })
        })
    };
    entered_rx.recv().expect("the background encode started");

    // The user's edit lands and the UI thread encodes it.
    cache.mark_dirty();
    assert_eq!(cache.get_shared(|| vec![1]).as_ref(), [1]);

    // Only now does the background encode finish, with its older bytes.
    release_tx.send(()).expect("release the background encode");
    background.join().expect("the background encode");

    assert_eq!(
        cache
            .get_shared(|| panic!("nothing changed; this must come from the cache"))
            .as_ref(),
        [1],
        "the stale background encode overwrote the newer state in the cache"
    );
}

/// The whole point of the cache: an unchanged document is not re-encoded.
#[test]
fn an_unchanged_document_is_served_from_the_cache() {
    let cache = EncodeCacheState::default();
    assert_eq!(cache.get_shared(|| vec![7]).as_ref(), [7]);
    assert_eq!(
        cache
            .get_shared(|| panic!("re-encoded a document that had not changed"))
            .as_ref(),
        [7]
    );
    cache.mark_dirty();
    assert_eq!(cache.get_shared(|| vec![8]).as_ref(), [8]);
}

/// Two threads encoding the same unchanged document must agree, and neither may
/// be handed a value the other invented.
#[test]
fn concurrent_readers_of_an_unchanged_document_agree() {
    let cache = Arc::new(EncodeCacheState::default());
    cache.mark_dirty();
    let readers = (0..8)
        .map(|_| {
            let cache = Arc::clone(&cache);
            std::thread::spawn(move || cache.get_shared(|| vec![42]))
        })
        .collect::<Vec<_>>();
    for reader in readers {
        assert_eq!(reader.join().expect("reader").as_ref(), [42]);
    }
}
