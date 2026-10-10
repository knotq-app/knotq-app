//! Where an `AppState`'s memory goes, by component, on a synthetic workspace.
//!
//! `scale_probe` answers "how long"; this answers "how much". A counting
//! allocator reports the bytes each piece still holds once built (not the
//! transient cost of building it), so a second copy of the workspace hiding in
//! some cache shows up as a multiple of the first line.
//!
//! Ignored by default because it is a measurement, not an assertion:
//!
//! ```sh
//! cargo test -p knotq-state --test memory_probe --release -- --ignored --nocapture
//! ```
//!
//! The one thing here that IS asserted lives in `guards`, at the bottom: a
//! state must not hold copies of the workspace it was built from.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use knotq_index::WorkspaceIndex;
use std::collections::HashMap;

use knotq_model::{AppSettings, DocumentId, Item, NodeRef, Scheme, Workspace};
use knotq_state::AppState;
use knotq_sync::WorkspaceCrdtDocuments;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
        PEAK.fetch_max(live, Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Bytes still held by what `build` returns, and the most held while building.
fn held<T>(build: impl FnOnce() -> T) -> (T, usize, usize) {
    let before = LIVE.load(Relaxed);
    PEAK.store(before, Relaxed);
    let value = build();
    let held = LIVE.load(Relaxed).saturating_sub(before);
    let peak = PEAK.load(Relaxed).saturating_sub(before);
    (value, held, peak)
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn synthetic_workspace(schemes: usize, items_per_scheme: usize, text_len: usize) -> Workspace {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let body = "lorem ipsum dolor sit amet ".repeat(text_len / 27 + 1);
    for scheme_index in 0..schemes {
        let mut scheme = Scheme::new(format!("scheme-{scheme_index}"), 0);
        for item_index in 0..items_per_scheme {
            let mut item = Item::new(format!(
                "{item_index} {}",
                &body[..text_len.min(body.len())]
            ));
            if item_index % 10 == 0 {
                item = item.with_start(chrono::Utc::now());
            }
            scheme.items.push(item);
        }
        let id = scheme.id;
        workspace.schemes.insert(id, scheme);
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(id));
    }
    workspace.ensure_sync_metadata();
    workspace
}

fn state_for(workspace: &Workspace) -> AppState {
    let crdt_states = WorkspaceCrdtDocuments::try_new(workspace)
        .expect("build crdt")
        .document_states();
    let today = chrono::Local::now().date_naive();
    AppState::new(
        workspace.clone(),
        AppSettings::default(),
        today,
        today,
        false,
        crdt_states,
        0,
    )
}

/// An `AppState` for a device that has never synced: no saved CRDT state, so
/// every document is unpopulated and the state keeps what each scheme held as
/// its population base.
fn unsynced_state_for(workspace: &Workspace) -> AppState {
    let today = chrono::Local::now().date_naive();
    AppState::new(
        workspace.clone(),
        AppSettings::default(),
        today,
        today,
        false,
        HashMap::<DocumentId, Vec<u8>>::new(),
        0,
    )
}

struct Measured {
    workspace: usize,
    unsynced_state: usize,
}

fn measure(schemes: usize, items_per_scheme: usize, text_len: usize, print: bool) -> Measured {
    let source = synthetic_workspace(schemes, items_per_scheme, text_len);
    // A deep copy, so the figure is what the lines themselves cost rather than
    // a reference count on `source`'s.
    let (copy, workspace_bytes, _) = held(|| deep_copy(&source));
    let (index, index_bytes, _) = held(|| WorkspaceIndex::build(&copy));
    drop(index);
    drop(copy);
    let (crdt, crdt_bytes, crdt_peak) =
        held(|| WorkspaceCrdtDocuments::try_new(&source).expect("build crdt"));
    let (states, states_bytes, _) = held(|| crdt.document_states());
    drop(states);
    drop(crdt);
    let (state, state_bytes, state_peak) = held(|| state_for(&source));
    drop(state);
    let (unsynced, unsynced_bytes, unsynced_peak) = held(|| unsynced_state_for(&source));
    drop(unsynced);
    if print {
        println!(
            "{schemes:>4} x {items_per_scheme:>4} x {text_len:>3}ch | workspace {:>6.1} MB | index {:>5.1} | \
             crdt docs {:>6.1} (peak {:>6.1}) | saved states {:>5.1} | \
             synced AppState +{:>6.1} (peak {:>6.1}) | never-synced AppState +{:>5.1} (peak {:>5.1})",
            mb(workspace_bytes),
            mb(index_bytes),
            mb(crdt_bytes),
            mb(crdt_peak),
            mb(states_bytes),
            mb(state_bytes),
            mb(state_peak),
            mb(unsynced_bytes),
            mb(unsynced_peak),
        );
    }
    Measured {
        workspace: workspace_bytes,
        unsynced_state: unsynced_bytes,
    }
}

/// A copy of `workspace` that shares nothing with it.
fn deep_copy(workspace: &Workspace) -> Workspace {
    let mut copy = workspace.clone();
    for scheme in copy.schemes.values_mut() {
        scheme.items = scheme.items.to_vec().into();
    }
    copy
}

#[test]
#[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
fn probe_memory() {
    println!();
    println!(
        "== bytes held, by component (AppState figures are on top of the caller's workspace) =="
    );
    for (schemes, items, text_len) in [
        (20, 50, 60),
        (200, 100, 60),
        (50, 1000, 60),
        (200, 100, 300),
    ] {
        measure(schemes, items, text_len, true);
    }
}

mod guards {
    use super::*;

    /// A state built from a workspace must not hold copies of its lines.
    ///
    /// It keeps the workspace three ways — the store's, the read-only view the
    /// app renders from, and (until a first sync) what every scheme held
    /// beforehand — and each used to be a full copy, so a never-synced install
    /// held its workspace several times over for the whole session. They share
    /// one set of lines now, and the state's own share is bookkeeping.
    #[test]
    fn a_never_synced_state_does_not_duplicate_the_workspace() {
        let measured = measure(40, 200, 60, false);
        assert!(
            measured.unsynced_state < measured.workspace / 2,
            "an AppState over a {:.1} MB workspace holds {:.1} MB of its own; \
             something is copying the lines again",
            mb(measured.workspace),
            mb(measured.unsynced_state),
        );
    }
}
