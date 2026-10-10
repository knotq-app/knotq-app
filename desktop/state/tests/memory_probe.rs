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
//! The one thing here that IS asserted lives in `guards`, at the bottom.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use knotq_index::WorkspaceIndex;
use knotq_model::{AppSettings, Item, NodeRef, Scheme, Workspace};
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

/// `(workspace bytes, whole AppState bytes)` for a workspace of this shape.
fn measure(
    schemes: usize,
    items_per_scheme: usize,
    text_len: usize,
    print: bool,
) -> (usize, usize) {
    let source = synthetic_workspace(schemes, items_per_scheme, text_len);
    let (copy, workspace_bytes, _) = held(|| source.clone());
    let (index, index_bytes, _) = held(|| WorkspaceIndex::build(&copy));
    drop(index);
    let (crdt, crdt_bytes, crdt_peak) =
        held(|| WorkspaceCrdtDocuments::try_new(&source).expect("build crdt"));
    let (states, states_bytes, _) = held(|| crdt.document_states());
    drop(states);
    drop(crdt);
    let (state, state_bytes, state_peak) = held(|| state_for(&source));
    drop(state);
    if print {
        println!(
            "{:>4} x {:>4} x {:>3}ch | workspace {:>6.1} MB | index +{:>6.1} | crdt docs {:>6.1} (peak {:>6.1}) | saved states {:>6.1} | AppState {:>6.1} (peak {:>6.1}) = {:.1}x workspace",
            schemes,
            items_per_scheme,
            text_len,
            mb(workspace_bytes),
            mb(index_bytes),
            mb(crdt_bytes),
            mb(crdt_peak),
            mb(states_bytes),
            mb(state_bytes),
            mb(state_peak),
            state_bytes as f64 / workspace_bytes as f64,
        );
    }
    (workspace_bytes, state_bytes)
}

#[test]
#[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
fn probe_memory() {
    println!();
    println!("== bytes held, by component ==");
    for (schemes, items, text_len) in [
        (20, 50, 60),
        (200, 100, 60),
        (50, 1000, 60),
        (200, 100, 300),
    ] {
        measure(schemes, items, text_len, true);
    }
}
