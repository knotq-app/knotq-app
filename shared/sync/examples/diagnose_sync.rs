//! Read-only diagnostic: run the server's CRDT validator against a live desktop
//! `sync-state.json` + `sync-crdt-state.json` to pinpoint why pushes are rejected.
//!
//! Usage (paths via env, defaults to the macOS data dir):
//!   KNOTQ_DIR="$HOME/Library/Application Support/KnotQ" \
//!     cargo run -p knotq-sync --example diagnose_sync
//!
//! It NEVER writes anything.

use std::collections::HashMap;

use base64::{engine::general_purpose::STANDARD, Engine};
use knotq_model::SyncDocumentKind;
use knotq_sync::validate_crdt_update_sequence;
use serde_json::Value;

fn kind_of(s: &str) -> Option<SyncDocumentKind> {
    match s {
        "scheme" => Some(SyncDocumentKind::Scheme),
        "personal_workspace" => Some(SyncDocumentKind::PersonalWorkspace),
        "folder" => Some(SyncDocumentKind::Folder),
        _ => None,
    }
}

fn main() {
    let dir = std::env::var("KNOTQ_DIR").unwrap_or_else(|_| {
        format!(
            "{}/Library/Application Support/KnotQ",
            std::env::var("HOME").unwrap()
        )
    });
    let sync_state: Value =
        serde_json::from_slice(&std::fs::read(format!("{dir}/sync-state.json")).unwrap()).unwrap();
    let crdt_state: Value =
        serde_json::from_slice(&std::fs::read(format!("{dir}/sync-crdt-state.json")).unwrap())
            .unwrap();

    // doc id -> kind, from cursors then pending.
    let mut kinds: HashMap<String, SyncDocumentKind> = HashMap::new();
    if let Some(cur) = sync_state
        .get("document_cursors")
        .and_then(|v| v.as_object())
    {
        for (doc, c) in cur {
            if let Some(k) = c.get("kind").and_then(|v| v.as_str()).and_then(kind_of) {
                kinds.insert(doc.clone(), k);
            }
        }
    }
    let workspace_id = sync_state
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // doc id -> base state_v1 bytes.
    let mut base: HashMap<String, Vec<u8>> = HashMap::new();
    for d in crdt_state["documents"].as_array().unwrap() {
        let id = d["document"].as_str().unwrap().to_string();
        let bytes = STANDARD.decode(d["state_v1"].as_str().unwrap()).unwrap();
        base.insert(id, bytes);
    }

    // pending updates grouped by doc, in file order.
    let mut pending: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    for e in sync_state["pending"].as_array().unwrap() {
        let id = e["document"].as_str().unwrap().to_string();
        if !kinds.contains_key(&id) {
            if let Some(k) = e.get("kind").and_then(|v| v.as_str()).and_then(kind_of) {
                kinds.insert(id.clone(), k);
            }
        }
        let bytes = STANDARD.decode(e["update_v1"].as_str().unwrap()).unwrap();
        pending.entry(id).or_default().push(bytes);
    }

    let kind_for = |id: &str| -> SyncDocumentKind {
        kinds.get(id).copied().unwrap_or(if id == workspace_id {
            SyncDocumentKind::PersonalWorkspace
        } else {
            SyncDocumentKind::Scheme
        })
    };

    // 1) Validate every LIVE local doc standalone (is the on-disk CRDT itself valid?).
    println!(
        "=== standalone validation of {} local CRDT docs ===",
        base.len()
    );
    let mut bad_base = 0;
    for (id, bytes) in &base {
        let kind = kind_for(id);
        if let Err(e) = validate_crdt_update_sequence(kind, [bytes.as_slice()]) {
            bad_base += 1;
            println!("  INVALID local doc {id} ({kind:?}): {e:#}");
        }
    }
    if bad_base == 0 {
        println!("  all local CRDT docs are individually VALID");
    }

    // 2) For docs with pending, simulate the server push: base + pending updates.
    println!("\n=== server-push simulation (local base + pending updates) ===");
    for (id, updates) in &pending {
        let kind = kind_for(id);
        let mut chain: Vec<&[u8]> = Vec::new();
        if let Some(b) = base.get(id) {
            chain.push(b.as_slice());
        }
        for u in updates {
            chain.push(u.as_slice());
        }
        let had_base = base.contains_key(id);
        match validate_crdt_update_sequence(kind, chain.iter().copied()) {
            Ok(()) => println!(
                "  OK    {id} ({kind:?}) base+{} pending -> valid (had_base={had_base})",
                updates.len()
            ),
            Err(e) => println!(
                "  REJECT {id} ({kind:?}) base+{} pending -> {e:#} (had_base={had_base})",
                updates.len()
            ),
        }
        // Also: pending-only (no base), the server's view if it had no base.
        if had_base {
            let only: Vec<&[u8]> = updates.iter().map(|u| u.as_slice()).collect();
            if let Err(e) = validate_crdt_update_sequence(kind, only.into_iter()) {
                println!("        (pending-only, no base) -> {e:#}");
            }
        }
    }
}
