//! Emit the canonical CRDT wire payloads the desktop client produces, so the
//! backend test suite can assert the TypeScript server validators accept exactly
//! what Rust generates (catching client/server schema drift).
//!
//! Regenerate the fixture with:
//!   cargo run -p knotq-sync --example emit_conformance_fixtures \
//!     > backend/cloudflare/test/fixtures/conformance.json

use knotq_model::{Folder, FolderId, Item, NodeRef, Scheme, Workspace};
use knotq_sync::WorkspaceCrdtDocuments;

fn main() -> anyhow::Result<()> {
    let mut workspace = Workspace::new();

    // A subfolder under the root so the workspace document carries a non-root
    // folder node in addition to the root.
    let folder = Folder {
        id: FolderId::new(),
        name: "Projects".to_string(),
        parent: Some(workspace.root),
        children: Vec::new(),
        expanded: true,
    };
    let folder_id = folder.id;

    // A scheme with a few items, nested under the subfolder.
    let mut scheme = Scheme::new("Conformance Plan", 0);
    scheme.items.push(Item::new("First line"));
    scheme.items.push(Item::new("Second line"));
    let scheme_id = scheme.id;

    workspace
        .folders
        .get_mut(&workspace.root)
        .expect("root folder")
        .children
        .push(NodeRef::Folder(folder_id));
    let mut folder = folder;
    folder.children.push(NodeRef::Scheme(scheme_id));
    workspace.folders.insert(folder_id, folder);
    workspace.schemes.insert(scheme_id, scheme);
    workspace.ensure_sync_metadata();

    // Full self-contained snapshot updates — exactly what the client pushes on a
    // bootstrap sync. CrdtDocumentUpdate serializes update_v1 as base64, matching
    // the push request wire format the worker expects.
    let updates = WorkspaceCrdtDocuments::snapshot_updates(&workspace).updates;
    println!("{}", serde_json::to_string_pretty(&updates)?);
    Ok(())
}
