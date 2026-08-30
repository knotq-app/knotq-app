//! What a save costs after an ordinary edit, full vs narrowed. Probe, not a test.
use std::collections::HashMap;
use std::time::Instant;

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{Item, NodeRef, ReplicaId, Scheme, Workspace};
use knotq_state::{CrdtSaveScope, WorkspaceStore};
use knotq_sync::WorkspaceCrdtDocuments;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore]
fn save_cost_full_vs_narrowed() {
    const SCHEMES: usize = 180;
    const ITEMS: usize = 60;

    let mut workspace = Workspace::new();
    let mut ids = Vec::new();
    for s in 0..SCHEMES {
        let mut scheme = Scheme::new(format!("Scheme {s}"), 0);
        for i in 0..ITEMS {
            scheme.items.push(Item::new(format!(
                "scheme {s} line {i} with some body text"
            )));
        }
        ids.push((scheme.id, scheme.items[0].id));
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme.id));
        workspace.schemes.insert(scheme.id, scheme);
    }
    workspace.ensure_sync_metadata();
    let seeded = WorkspaceCrdtDocuments::try_new(&workspace)
        .unwrap()
        .document_states();
    let mut store = WorkspaceStore::new(workspace, ReplicaId::new(), false, seeded, 1);

    let path = std::env::temp_dir().join(format!("knotq-save-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();

    // First save: full, as a run always starts.
    let (scope, handles) = store.take_crdt_save_scope();
    assert_eq!(scope, CrdtSaveScope::All);
    let states: HashMap<_, _> = handles.into_iter().map(|(d, h)| (d, h.encode())).collect();
    let total: usize = states.values().map(|b| b.len()).sum();
    knotq_storage_json::save_crdt_state(&path, &states).unwrap();
    println!(
        "\n{SCHEMES} schemes x {ITEMS} items = {} documents, {:.1} MB of state",
        states.len(),
        total as f64 / 1e6
    );

    for round in 0..3 {
        // One keystroke.
        let (scheme, item) = ids[round % ids.len()];
        store
            .apply_local(
                Command::UpdateItemText {
                    scheme,
                    item,
                    text: format!("edited {round}"),
                },
                CommandOrigin::User,
            )
            .unwrap();

        // What the save would do today: every document.
        let all = store.crdt_document_state_handles();
        let start = Instant::now();
        let states: HashMap<_, _> = all.into_iter().map(|(d, h)| (d, h.encode())).collect();
        let encode_all = ms(start);
        let start = Instant::now();
        knotq_storage_json::save_crdt_state(&path, &states).unwrap();
        let write_all = ms(start);

        // What it does now.
        store
            .apply_local(
                Command::UpdateItemText {
                    scheme,
                    item,
                    text: format!("edited {round} again"),
                },
                CommandOrigin::User,
            )
            .unwrap();
        let (scope, handles) = store.take_crdt_save_scope();
        let named = handles.len();
        let start = Instant::now();
        let states: HashMap<_, _> = handles.into_iter().map(|(d, h)| (d, h.encode())).collect();
        let encode_some = ms(start);
        let start = Instant::now();
        match scope {
            CrdtSaveScope::All => knotq_storage_json::save_crdt_state(&path, &states).unwrap(),
            CrdtSaveScope::Only(_) => {
                knotq_storage_json::save_crdt_state_incremental(&path, &states).unwrap()
            }
        }
        let write_some = ms(start);

        println!(
            "round {round}: full  encode {encode_all:6.2} ms  write {write_all:6.2} ms   |   \
             narrowed ({named} doc) encode {encode_some:6.2} ms  write {write_some:6.2} ms"
        );
    }
    let _ = std::fs::remove_dir_all(&path);
}
