//! A long-running keystroke loop, so a sampling profiler has something to
//! sample. Not an assertion — `#[ignore]`d.
//!
//! ```sh
//! cargo test -p knotq-state --test keystroke_spin --release -- --ignored --nocapture &
//! sample $! 20 -file /tmp/keystroke.txt
//! ```
use std::time::{Duration, Instant};

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{AppSettings, Item, NodeRef, Scheme, Workspace};
use knotq_state::AppState;
use knotq_sync::WorkspaceCrdtDocuments;

#[test]
#[ignore]
fn spin() {
    let items: usize = std::env::var("KNOTQ_SPIN_ITEMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let seconds: u64 = std::env::var("KNOTQ_SPIN_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let mut workspace = Workspace::new();
    let root = workspace.root;
    let mut scheme = Scheme::new("spin", 0);
    for index in 0..items {
        let mut item = Item::new("");
        item.set_text(format!(
            "{index} lorem ipsum dolor sit amet consectetur adipiscing elit sed do"
        ));
        scheme.items.push(item);
    }
    let scheme_id = scheme.id;
    let item_id = scheme.items[0].id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace.ensure_sync_metadata();

    let crdt_states = WorkspaceCrdtDocuments::try_new(&workspace)
        .expect("build crdt")
        .document_states();
    let today = chrono::Local::now().date_naive();
    let mut state = AppState::new(
        workspace,
        AppSettings::default(),
        today,
        today,
        false,
        crdt_states,
        0,
    );

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut keystrokes = 0u64;
    while Instant::now() < deadline {
        for _ in 0..50 {
            let _ = state.apply_prechecked_local_command(
                Command::UpdateItemText {
                    scheme: scheme_id,
                    item: item_id,
                    text: format!("edited {keystrokes}"),
                },
                CommandOrigin::User,
            );
            keystrokes += 1;
        }
    }
    println!("{keystrokes} keystrokes on {items} items");
}
