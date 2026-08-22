//! Diagnostic for a real data directory: does its persisted CRDT state still
//! rebuild and materialize? Reports rather than asserts.
//!
//! KNOTQ_DIAG_DIR=~/Library/Application\ Support/KnotQ \
//!   cargo test -p knotq-state --test diagnose_workspace -- --ignored --nocapture

use knotq_model::ReplicaId;
use knotq_sync::WorkspaceCrdtDocuments;

#[test]
#[ignore = "diagnostic; needs KNOTQ_DIAG_DIR"]
fn diagnose() {
    let dir = std::path::PathBuf::from(std::env::var("KNOTQ_DIAG_DIR").expect("KNOTQ_DIAG_DIR"));
    let ws_path = dir.join("workspace/workspace.json");
    let workspace = knotq_storage_json::load_workspace(&ws_path)
        .expect("load workspace")
        .expect("workspace present");
    println!("workspace id       : {}", workspace.id);
    println!("workspace sync doc : {}", workspace.sync.id);
    println!("schemes on disk    : {}", workspace.schemes.len());

    let states = knotq_storage_json::load_crdt_state(&ws_path).expect("load crdt state");
    println!("crdt documents     : {}", states.len());
    let ws_doc = states.get(&workspace.sync.id);
    println!(
        "workspace doc      : {}",
        match ws_doc {
            Some(b) if b.is_empty() => "PRESENT BUT EMPTY".to_string(),
            Some(b) => format!("{} bytes", b.len()),
            None => "MISSING".to_string(),
        }
    );

    match WorkspaceCrdtDocuments::from_states(&workspace, ReplicaId::new(), &states) {
        Ok(docs) => {
            println!("from_states        : OK");
            // sync_changes exercises the same validation path a sync run uses.
            let mut docs = docs;
            let outcome =
                docs.sync_changes(&workspace, &knotq_sync::WorkspaceCrdtChangeSet::default());
            if outcome.is_ok() {
                println!("validation         : OK");
            } else {
                println!("validation         : errors: {:?}", outcome.errors);
            }
            // Rebuild the workspace purely from the CRDT and compare against
            // what is on disk: this is what a sync run materializes from, so a
            // mismatch here is what the app would show as lost or wrong content.
            match docs.materialized_workspace_for_diagnostics(&workspace) {
                Ok(rebuilt) => {
                    println!(
                        "materialized       : {} schemes, {} folders (disk: {} / {})",
                        rebuilt.schemes.len(),
                        rebuilt.folders.len(),
                        workspace.schemes.len(),
                        workspace.folders.len()
                    );
                    let mut missing = 0;
                    let mut item_delta = 0i64;
                    for (id, disk) in &workspace.schemes {
                        match rebuilt.schemes.get(id) {
                            None => missing += 1,
                            Some(r) => {
                                item_delta +=
                                    r.items.len() as i64 - disk.items.len() as i64;
                            }
                        }
                    }
                    // Daily Queue schemes live in daily_queue/ files and are
                    // deliberately absent from the workspace index document, so
                    // they are expected here and are not a sign of damage.
                    println!("schemes not in the workspace index: {missing} (Daily Queue schemes are expected)");
                    for (id, disk) in &workspace.schemes {
                        if !rebuilt.schemes.contains_key(id) {
                            let doc = workspace.scheme_sync.get(id).map(|m| m.id);
                            println!(
                                "  missing: {:?} name={:?} items={} doc={:?} doc_on_disk={}",
                                id,
                                disk.name,
                                disk.items.len(),
                                doc,
                                doc.map(|d| states.contains_key(&d)).unwrap_or(false)
                            );
                        }
                    }
                    println!("total item count delta   : {item_delta}");
                }
                Err(e) => println!("materialized       : FAILED: {e:#}"),
            }
        }
        Err(e) => println!("from_states        : FAILED: {e:#}"),
    }
}
