use std::ops::Deref;

use knotq_model::Workspace;

/// Read-only access to the live workspace.
///
/// Everything that changes what the user sees must go through the store — a
/// [`Command`](knotq_commands::Command), a sync landing, or one of the few
/// named `AppState` entry points (loading a day from disk, index repair) — so
/// that every change reaches the CRDT documents the same way and there is one
/// path to test. This type derefs to `&Workspace` and never to
/// `&mut Workspace`, so a direct mutation from app code does not compile.
pub struct WorkspaceView(Workspace);

impl WorkspaceView {
    pub(crate) fn new(workspace: Workspace) -> Self {
        Self(workspace)
    }

    pub(crate) fn get_mut(&mut self) -> &mut Workspace {
        &mut self.0
    }
}

impl Deref for WorkspaceView {
    type Target = Workspace;

    fn deref(&self) -> &Workspace {
        &self.0
    }
}

impl PartialEq<Workspace> for WorkspaceView {
    fn eq(&self, other: &Workspace) -> bool {
        self.0 == *other
    }
}

impl std::fmt::Debug for WorkspaceView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
