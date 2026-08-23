//! Writes a synthetic data directory for hand-measuring the app.
//!
//! Not an assertion — a fixture generator, so it is `#[ignore]`d.
//!
//! ```sh
//! KNOTQ_SEED_DIR=/tmp/knotq-probe KNOTQ_SEED_ITEMS=2000 \
//!   cargo test -p knotq-storage-json --test seed_probe_workspace -- --ignored --nocapture
//! ```

use knotq_model::{Item, ItemMarker, NodeRef, Scheme, Workspace};

#[test]
#[ignore]
fn seed() {
    let dir = std::env::var("KNOTQ_SEED_DIR").expect("KNOTQ_SEED_DIR");
    let items: usize = std::env::var("KNOTQ_SEED_ITEMS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2_000);
    let dir = std::path::PathBuf::from(dir).join("workspace");
    std::fs::create_dir_all(&dir).expect("create workspace dir");

    let mut workspace = Workspace::new();
    let root = workspace.root;
    let mut scheme = Scheme::new(format!("Probe {items}"), 0);
    for index in 0..items {
        let mut item = Item::new("");
        item.set_text(format!(
            "{index:04} lorem ipsum dolor sit amet consectetur adipiscing elit sed do"
        ));
        item.marker = if index % 3 == 0 {
            ItemMarker::Checkbox
        } else {
            ItemMarker::Bullet
        };
        item.indent = (index % 3) as u8;
        scheme.items.push(item);
    }
    let id = scheme.id;
    workspace.schemes.insert(id, scheme);
    workspace
        .folders
        .get_mut(&root)
        .expect("root folder")
        .children
        .push(NodeRef::Scheme(id));
    workspace.ensure_sync_metadata();

    knotq_storage_json::save_workspace(&dir.join("workspace.json"), &workspace)
        .expect("save workspace");
    println!("seeded {items} items into {}", dir.display());
}
