//! The durable save encodes each CRDT document's state on a background thread,
//! from a handle taken on the UI thread. Handle and document share one cache, so
//! that encode runs *concurrently with editing* — and the desktop rebuilds its
//! live CRDT documents from exactly these bytes (`WorkspaceStore::replace_workspace`,
//! the direct-mutation flush that runs before every sync landing), as well as
//! writing them to disk.
//!
//! So a document state that misses a local edit is not just a stale snapshot:
//! the edit is dropped from the document, and the next materialization puts the
//! line the user deleted back on screen.

use std::collections::HashMap;
use std::sync::{Arc, Barrier};

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{DocumentId, Item, ItemId, NodeRef, ReplicaId, Scheme, SchemeId, Workspace};
use knotq_state::{WorkspaceDirtyState, WorkspaceStore};
use knotq_sync::WorkspaceCrdtDocuments;

/// Enough filler that encoding the document's full state takes long enough for
/// the editing thread to read the cache while the background encode is still
/// inside it. That window is the whole bug; with a three-line scheme the encode
/// finishes before anything can observe it.
const FILLER_LINES: usize = 2000;

fn store_with_lines(lines: &[&str]) -> (WorkspaceStore, SchemeId, Vec<ItemId>) {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Notes", 0);
    for n in 0..FILLER_LINES {
        scheme.items.push(Item::new(format!("filler line {n}")));
    }
    let ids = lines
        .iter()
        .map(|line| {
            let item = Item::new(*line);
            let id = item.id;
            scheme.items.push(item);
            id
        })
        .collect();
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace.ensure_sync_metadata();
    let seeded = WorkspaceCrdtDocuments::try_new(&workspace)
        .unwrap()
        .document_states();
    let store = WorkspaceStore::new(workspace, ReplicaId::new(), false, seeded, 1);
    (store, scheme_id, ids)
}

/// What a restart — or the CRDT rebuild inside `replace_workspace` — would see:
/// the scheme's lines according to `states` alone.
fn lines_from_states(
    workspace: &Workspace,
    scheme: SchemeId,
    states: &HashMap<DocumentId, Arc<[u8]>>,
) -> Vec<String> {
    let docs = WorkspaceCrdtDocuments::from_states(workspace, ReplicaId::new(), states)
        .expect("rebuild the documents from their persisted state");
    docs.materialized_workspace_for_diagnostics(workspace)
        .expect("materialize")
        .scheme(scheme)
        .expect("scheme")
        .items
        .iter()
        .map(|item| item.text())
        .collect()
}

fn delete_last(store: &mut WorkspaceStore, scheme: SchemeId) {
    let item = store
        .workspace()
        .scheme(scheme)
        .unwrap()
        .items
        .last()
        .unwrap()
        .id;
    store
        .apply_local(Command::DeleteItem { scheme, item }, CommandOrigin::User)
        .unwrap();
}

fn scheme_lines(store: &WorkspaceStore, scheme: SchemeId) -> Vec<String> {
    store
        .workspace()
        .scheme(scheme)
        .unwrap()
        .items
        .iter()
        .map(|item| item.text())
        .collect()
}

/// Encode the handles on another thread, exactly as the save task does, starting
/// when the returned barrier is released.
fn background_encode(
    handles: HashMap<DocumentId, knotq_sync::DocumentStateHandle>,
) -> (
    Arc<Barrier>,
    std::thread::JoinHandle<HashMap<DocumentId, Arc<[u8]>>>,
) {
    let gate = Arc::new(Barrier::new(2));
    let thread_gate = Arc::clone(&gate);
    let joined = std::thread::spawn(move || {
        thread_gate.wait();
        handles
            .into_iter()
            .map(|(document, handle)| (document, handle.encode()))
            .collect()
    });
    (gate, joined)
}

/// The document states the store hands out must describe the workspace it is
/// holding, even while the save task is encoding handles it took a moment ago.
///
/// The shape that breaks it: the cache holds the state from the last save, the
/// user deletes some lines, and the background encode of those deletes starts
/// before the editing thread asks for the states again.
#[test]
fn a_reader_never_gets_a_pre_delete_state_while_a_save_is_encoding() {
    for attempt in 0..8 {
        let (mut store, scheme, _) =
            store_with_lines(&["one", "two", "three", "four", "five", "six"]);
        let workspace_before = store.workspace().clone();
        // The previous save populated the cache.
        let _ = store.crdt_document_states();

        // This save's snapshot: handles on the UI thread, encoding in the background.
        let (gate, encoding) = background_encode(store.crdt_document_state_handles());

        // The user deletes four lines...
        for _ in 0..4 {
            delete_last(&mut store, scheme);
        }
        let expected = scheme_lines(&store, scheme);
        assert_eq!(&expected[FILLER_LINES..], ["one", "two"]);

        // ...and the background encode of them starts while the editing thread
        // is still asking for states.
        gate.wait();
        // Let the background encode get inside `encode` before reading. It is
        // holding the notice that the document changed; the reader must not
        // conclude from its absence that the cache is current. (A sleep only
        // *widens* the window — the fixed cache holds the invariant under every
        // interleaving, so this cannot make the test flaky, only less likely to
        // catch a regression if the machine is heavily loaded.)
        std::thread::sleep(std::time::Duration::from_millis(2));
        // Read repeatedly: the stale answer only comes back while the encode is
        // still running, so a single read after it finished would miss it.
        // Compared as bytes first — rebuilding a document per read is far more
        // expensive than the race itself — and materialized once, to say what
        // actually went wrong.
        let reads = (0..8)
            .map(|_| store.crdt_document_states())
            .collect::<Vec<_>>();
        let _ = encoding.join().expect("the background encode");

        for (read, states) in reads.iter().enumerate() {
            if states == &reads[0] {
                continue;
            }
            assert_eq!(
                lines_from_states(&workspace_before, scheme, states),
                expected,
                "attempt {attempt}, read {read}: the states the store handed out moved \
                 mid-race"
            );
        }
        assert_eq!(
            lines_from_states(&workspace_before, scheme, &reads[0]),
            expected,
            "attempt {attempt}: rebuilding the CRDT from the states the store handed \
             out resurrects a deleted line"
        );
    }
}

/// The same window from the save's side: the bytes the save writes to disk must
/// not be the pre-delete state, or the deleted lines come back on the next
/// launch.
///
/// Both threads ask for the state at the same instant. Whichever loses the race
/// to re-encode is the one that gets handed the cache — so over a few attempts
/// this covers the save being the loser as well as the editor.
#[test]
fn neither_the_save_nor_the_editor_is_handed_the_pre_delete_state() {
    for attempt in 0..8 {
        let (mut store, scheme, _) = store_with_lines(&["one", "two", "three", "four"]);
        let workspace_before = store.workspace().clone();
        let _ = store.crdt_document_states();

        let (gate, encoding) = background_encode(store.crdt_document_state_handles());

        for _ in 0..2 {
            delete_last(&mut store, scheme);
        }
        let expected = scheme_lines(&store, scheme);

        // Released together: one of the two does the encode, the other reads the
        // cache. Neither may be handed the state from before the deletes.
        gate.wait();
        let live = store.crdt_document_states();
        let saved = encoding.join().expect("the background encode");

        assert_eq!(
            lines_from_states(&workspace_before, scheme, &live),
            expected,
            "attempt {attempt}: the editing thread was handed the pre-delete state"
        );
        assert_eq!(
            lines_from_states(&workspace_before, scheme, &saved),
            expected,
            "attempt {attempt}: the save persisted the pre-delete state, so the \
             deleted lines come back on the next launch"
        );
    }
}

/// End to end, in the order the desktop actually runs it.
///
/// The user deletes some lines; the save task then takes its handles and
/// *clears* the dirty set (`std::mem::take(&mut app.state.dirty_schemes)`), so
/// the flush that runs before the next sync landing has nothing to re-sync. That
/// flush rebuilds the live CRDT documents from `document_states()` — with
/// nothing marked dirty, whatever those bytes say is what the scheme becomes.
#[test]
fn the_pre_sync_flush_keeps_the_deletes_after_the_save_cleared_the_dirty_set() {
    for attempt in 0..8 {
        let (mut store, scheme, _) = store_with_lines(&["one", "two", "three", "four", "five"]);
        let _ = store.crdt_document_states();

        // The user deletes three lines.
        for _ in 0..3 {
            delete_last(&mut store, scheme);
        }
        let expected = scheme_lines(&store, scheme);
        assert_eq!(&expected[FILLER_LINES..], ["one", "two"]);

        // The save task snapshots: handles on the UI thread, dirty set cleared,
        // encoding in the background.
        let (gate, encoding) = background_encode(store.crdt_document_state_handles());
        store.replace_dirty_state(WorkspaceDirtyState::default());
        gate.wait();
        std::thread::sleep(std::time::Duration::from_millis(2));

        // `AppState::sync_store_from_workspace` runs before every sync merge and
        // rebuilds the CRDT documents from their states. Nothing is dirty, so
        // nothing re-syncs the scheme afterwards.
        let workspace = store.workspace().clone();
        let dirty = store.dirty().clone();
        store.replace_workspace(workspace, dirty, false);
        let _ = encoding.join().expect("the background encode");

        assert_eq!(
            scheme_lines(&store, scheme),
            expected,
            "attempt {attempt}: the pre-sync flush resurrected a deleted line"
        );
        let workspace_now = store.workspace().clone();
        assert_eq!(
            lines_from_states(&workspace_now, scheme, &store.crdt_document_states()),
            expected,
            "attempt {attempt}: the rebuilt documents disagree with the workspace"
        );
    }
}
