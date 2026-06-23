use knotq_model::{SyncDocumentKind, Workspace};

use crate::SyncDocumentRef;

pub fn sync_documents(workspace: &Workspace) -> Vec<SyncDocumentRef> {
    let mut docs = vec![SyncDocumentRef {
        document: workspace.sync.id,
        kind: SyncDocumentKind::PersonalWorkspace,
    }];
    docs.extend(scheme_documents(workspace));
    docs
}

pub fn scheme_documents(workspace: &Workspace) -> Vec<SyncDocumentRef> {
    workspace
        .scheme_sync
        .values()
        .filter(|meta| meta.kind == SyncDocumentKind::Scheme)
        .map(|meta| SyncDocumentRef {
            document: meta.id,
            kind: SyncDocumentKind::Scheme,
        })
        .collect()
}
