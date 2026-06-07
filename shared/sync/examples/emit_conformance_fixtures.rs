//! Emit the canonical CRDT wire payloads the desktop client produces, so the
//! backend test suite can assert the TypeScript server validators accept exactly
//! what Rust generates (catching client/server schema drift).
//!
//! Regenerate the fixture with:
//!   cargo run -p knotq-sync --example emit_conformance_fixtures \
//!     > backend/cloudflare/test/fixtures/conformance.json

use knotq_model::{
    CrdtBackend, DocumentId, Folder, FolderId, Item, ItemId, NodeRef, Scheme, SchemeId, SyncAccess,
    SyncDocumentKind, SyncDocumentMeta, Workspace, WorkspaceId,
};
use knotq_sync::WorkspaceCrdtDocuments;
use uuid::Uuid;

fn main() -> anyhow::Result<()> {
    let mut workspace = Workspace::new();
    workspace.id = workspace_id("00000000-0000-4000-8000-000000000001");
    workspace.sync = sync_meta(
        document_id("00000000-0000-4000-8000-000000000002"),
        SyncDocumentKind::PersonalWorkspace,
    );
    workspace.root = folder_id("00000000-0000-4000-8000-000000000003");
    workspace.folders.clear();
    workspace.folders.insert(
        workspace.root,
        Folder {
            id: workspace.root,
            name: "root".into(),
            parent: None,
            children: Vec::new(),
            expanded: true,
        },
    );
    workspace.folder_sync.insert(
        workspace.root,
        sync_meta(
            document_id("00000000-0000-4000-8000-000000000004"),
            SyncDocumentKind::Folder,
        ),
    );

    // A subfolder under the root so the workspace document carries a non-root
    // folder node in addition to the root.
    let folder_id = folder_id("00000000-0000-4000-8000-000000000005");
    let folder = Folder {
        id: folder_id,
        name: "Projects".to_string(),
        parent: Some(workspace.root),
        children: Vec::new(),
        expanded: true,
    };
    workspace.folder_sync.insert(
        folder_id,
        sync_meta(
            document_id("00000000-0000-4000-8000-000000000006"),
            SyncDocumentKind::Folder,
        ),
    );

    // A scheme with a few items, nested under the subfolder.
    let scheme_id = scheme_id("00000000-0000-4000-8000-000000000007");
    let mut scheme = Scheme::new("Conformance Plan", 0);
    scheme.id = scheme_id;
    let mut first = Item::new("First line");
    first.id = item_id("00000000-0000-4000-8000-000000000009");
    let mut second = Item::new("Second line");
    second.id = item_id("00000000-0000-4000-8000-00000000000a");
    scheme.items.push(first);
    scheme.items.push(second);
    workspace.scheme_sync.insert(
        scheme_id,
        sync_meta(
            document_id("00000000-0000-4000-8000-000000000008"),
            SyncDocumentKind::Scheme,
        ),
    );

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
    let updates =
        WorkspaceCrdtDocuments::snapshot_updates_with_client_ids(&workspace, 1, |_, _| 2).updates;
    println!("{}", serde_json::to_string_pretty(&updates)?);
    Ok(())
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("valid fixed fixture uuid")
}

fn workspace_id(value: &str) -> WorkspaceId {
    WorkspaceId(uuid(value))
}

fn document_id(value: &str) -> DocumentId {
    DocumentId(uuid(value))
}

fn folder_id(value: &str) -> FolderId {
    FolderId(uuid(value))
}

fn scheme_id(value: &str) -> SchemeId {
    SchemeId(uuid(value))
}

fn item_id(value: &str) -> ItemId {
    ItemId(uuid(value))
}

fn sync_meta(id: DocumentId, kind: SyncDocumentKind) -> SyncDocumentMeta {
    SyncDocumentMeta {
        id,
        kind,
        crdt: CrdtBackend::Yrs,
        access: SyncAccess::Local,
        remote: None,
    }
}
