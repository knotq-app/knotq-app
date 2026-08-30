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
    // How many OTHER schemes surround the one being edited. A real workspace is
    // not one big scheme: the reference workspace this models has 75 schemes plus
    // 107 daily-queue days, and any per-keystroke cost that scales with scheme
    // COUNT is invisible in a one-scheme probe.
    let schemes: usize = std::env::var("KNOTQ_SEED_SCHEMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
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
    // Optionally tuck the filler schemes inside a COLLAPSED folder, so the
    // workspace still holds them but the sidebar renders none of their rows.
    // That separates "how many documents exist" from "how many rows are drawn".
    let hide = std::env::var("KNOTQ_SEED_HIDE").is_ok();
    let bucket = if hide {
        let id = knotq_model::FolderId::new();
        let folder = knotq_model::Folder {
            id,
            name: "Archive Bucket".to_string(),
            parent: Some(root),
            children: Vec::new(),
            expanded: false,
        };
        workspace.folders.insert(id, folder);
        workspace
            .folders
            .get_mut(&root)
            .expect("root folder")
            .children
            .push(NodeRef::Folder(id));
        id
    } else {
        root
    };
    for extra in 0..schemes {
        let mut other = Scheme::new(format!("Scheme {extra}"), (extra % 8) as u8);
        for index in 0..24 {
            let mut item = Item::new("");
            item.set_text(format!("{index} lorem ipsum dolor sit amet consectetur"));
            other.items.push(item);
        }
        let other_id = other.id;
        workspace.schemes.insert(other_id, other);
        workspace
            .folders
            .get_mut(&bucket)
            .expect("bucket folder")
            .children
            .push(NodeRef::Scheme(other_id));
    }
    workspace.ensure_sync_metadata();

    knotq_storage_json::save_workspace(&dir.join("workspace.json"), &workspace)
        .expect("save workspace");
    println!("seeded {items} items into {}", dir.display());
}
