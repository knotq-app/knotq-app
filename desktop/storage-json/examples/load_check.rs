//! Load a KnotQ workspace from disk and report what came back, to sanity-check
//! a migrated data directory.
//!
//! Usage:
//!   cargo run -p knotq-storage-json --example load_check -- <workspace.json>

use knotq_storage_json::load_workspace;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: load_check <workspace.json>");
        std::process::exit(2);
    };
    let workspace = match load_workspace(std::path::Path::new(&path)) {
        Ok(Some(ws)) => ws,
        Ok(None) => {
            eprintln!("no workspace at {path}");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("load failed: {err:#}");
            std::process::exit(1);
        }
    };

    let mut items = 0usize;
    let mut images = 0usize;
    let mut tables = 0usize;
    for scheme in workspace.schemes.values() {
        for item in &scheme.items {
            items += 1;
            images += item.images().count();
            tables += item.table().is_some() as usize;
        }
    }
    println!(
        "loaded ok: {} schemes, {} folders, {items} items, {images} inline images, {tables} tables",
        workspace.schemes.len(),
        workspace.folders.len(),
    );
}
