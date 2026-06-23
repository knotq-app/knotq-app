use gpui::Context;
use knotq_commands::Command;
use knotq_model::{FolderId, NodeRef, SchemeId};

use super::super::{KnotQApp, View};

impl KnotQApp {
    pub fn delete_folder(&mut self, folder_id: FolderId, cx: &mut Context<Self>) {
        if folder_id == self.workspace.root {
            return;
        }
        if self.workspace.folder(folder_id).is_none() {
            return;
        }
        if self
            .apply(Command::DeleteFolder { id: folder_id }, cx)
            .is_some()
        {
            self.trash_expanded = true;
            cx.notify();
        }
    }

    pub fn delete_scheme(&mut self, scheme_id: SchemeId, cx: &mut Context<Self>) {
        if self
            .workspace
            .scheme(scheme_id)
            .is_none_or(|_| self.workspace.is_daily_queue_scheme(scheme_id))
        {
            return;
        }

        let was_selected = self.selection.scheme_id == Some(scheme_id);
        let fallback = was_selected
            .then(|| self.first_visible_scheme_id_except(scheme_id))
            .flatten();

        if self
            .apply(Command::DeleteScheme { id: scheme_id }, cx)
            .is_none()
        {
            return;
        }

        self.trash_expanded = true;
        cx.notify();

        if self
            .scheme_editor
            .as_ref()
            .is_some_and(|(id, _)| *id == scheme_id)
        {
            self.scheme_editor = None;
            self._editor_subscription = None;
        }
        self.close_popovers_for_scheme(scheme_id);
        if was_selected {
            if let Some(next_id) = fallback {
                self.open_scheme(next_id, None);
            } else {
                self.open_union();
                self.selection.scheme_id = None;
                self.selection.focused_item_id = None;
            }
            cx.notify();
        }
    }

    pub fn restore_deleted_scheme(&mut self, scheme_id: SchemeId, cx: &mut Context<Self>) {
        if !self.workspace.is_scheme_deleted(scheme_id) {
            return;
        }
        let Some(scheme) = self.workspace.scheme(scheme_id).cloned() else {
            self.workspace.unmark_scheme_deleted(scheme_id);
            self.state.mark_index_dirty();
            self.service_bus.signal_save();
            cx.notify();
            return;
        };
        let (folder, position) = self.deleted_scheme_restore_target(scheme_id);
        if self
            .apply(
                Command::RestoreScheme {
                    folder,
                    position,
                    scheme,
                },
                cx,
            )
            .is_some()
        {
            self.open_scheme(scheme_id, None);
            cx.notify();
        }
    }

    pub fn permanently_delete_scheme(&mut self, scheme_id: SchemeId, cx: &mut Context<Self>) {
        if !self.workspace.is_scheme_deleted(scheme_id) {
            return;
        }
        if self
            .apply(Command::PermanentlyDeleteScheme { id: scheme_id }, cx)
            .is_none()
        {
            return;
        }
        self.scheme_sessions.remove(&scheme_id);
        if self
            .scheme_editor
            .as_ref()
            .is_some_and(|(id, _)| *id == scheme_id)
        {
            self.scheme_editor = None;
            self._editor_subscription = None;
        }
        if self.selection.scheme_id == Some(scheme_id) {
            self.selection.scheme_id = None;
            self.selection.focused_item_id = None;
            if matches!(self.selection.view, View::Scheme) {
                self.open_union();
            }
        }
        cx.notify();
    }

    pub fn restore_deleted_folder(&mut self, folder_id: FolderId, cx: &mut Context<Self>) {
        if !self.workspace.is_folder_deleted(folder_id) {
            return;
        }
        let Some(folder) = self.workspace.folder(folder_id).cloned() else {
            self.workspace.unmark_folder_deleted_shallow(folder_id);
            self.state.mark_index_dirty();
            self.service_bus.signal_save();
            cx.notify();
            return;
        };
        let (parent, position) = self.deleted_folder_restore_target(folder_id);
        if self
            .apply(
                Command::RestoreFolder {
                    parent,
                    position,
                    folder,
                },
                cx,
            )
            .is_some()
        {
            cx.notify();
        }
    }

    pub fn permanently_delete_folder(&mut self, folder_id: FolderId, cx: &mut Context<Self>) {
        if !self.workspace.is_folder_deleted(folder_id) {
            return;
        }
        let removed_schemes = self
            .workspace
            .subtree_scheme_ids(folder_id)
            .into_iter()
            .collect::<Vec<_>>();
        if self
            .apply(Command::PermanentlyDeleteFolder { id: folder_id }, cx)
            .is_none()
        {
            return;
        }
        for scheme_id in removed_schemes {
            self.scheme_sessions.remove(&scheme_id);
            if self
                .scheme_editor
                .as_ref()
                .is_some_and(|(id, _)| *id == scheme_id)
            {
                self.scheme_editor = None;
                self._editor_subscription = None;
            }
            if self.selection.scheme_id == Some(scheme_id) {
                self.selection.scheme_id = None;
                self.selection.focused_item_id = None;
                if matches!(self.selection.view, View::Scheme) {
                    self.open_union();
                }
            }
        }
        cx.notify();
    }

    pub fn empty_archive(&mut self, cx: &mut Context<Self>) {
        let deleted_folders = self.workspace.recently_deleted_folders.clone();
        let deleted = self.workspace.recently_deleted.clone();
        let standalone_deleted = deleted
            .iter()
            .copied()
            .filter(|id| !self.workspace.is_scheme_in_deleted_folder_subtree(*id))
            .collect::<Vec<_>>();
        if deleted_folders.is_empty() && standalone_deleted.is_empty() {
            return;
        }
        let mut commands = deleted_folders
            .iter()
            .copied()
            .map(|id| Command::PermanentlyDeleteFolder { id })
            .collect::<Vec<_>>();
        commands.extend(
            standalone_deleted
                .iter()
                .copied()
                .map(|id| Command::PermanentlyDeleteScheme { id }),
        );
        let Some(command) = Command::from_vec(commands) else {
            return;
        };
        if self.apply(command, cx).is_none() {
            return;
        }
        for id in deleted {
            self.scheme_sessions.remove(&id);
            if self
                .scheme_editor
                .as_ref()
                .is_some_and(|(editor_id, _)| *editor_id == id)
            {
                self.scheme_editor = None;
                self._editor_subscription = None;
            }
        }
        if let Some(selected) = self.selection.scheme_id {
            if self.workspace.scheme(selected).is_none() {
                self.selection.scheme_id = None;
                self.selection.focused_item_id = None;
                if matches!(self.selection.view, View::Scheme) {
                    self.open_union();
                }
            }
        }
        cx.notify();
    }

    fn deleted_scheme_restore_target(&self, scheme_id: SchemeId) -> (FolderId, usize) {
        if let Some(origin) = self.workspace.deleted_scheme_origin(scheme_id) {
            if self.is_valid_scheme_restore_folder(origin.folder) {
                let len = self
                    .workspace
                    .folder(origin.folder)
                    .map(|folder| folder.children.len())
                    .unwrap_or(0);
                return (origin.folder, origin.position.min(len));
            }
        }

        let root = self.workspace.root;
        let position = self
            .workspace
            .folder(root)
            .map(|folder| folder.children.len())
            .unwrap_or(0);
        (root, position)
    }

    fn deleted_folder_restore_target(&self, folder_id: FolderId) -> (FolderId, usize) {
        if let Some(origin) = self.workspace.deleted_folder_origin(folder_id) {
            if self.is_valid_folder_restore_parent(origin.parent) {
                let len = self
                    .workspace
                    .folder(origin.parent)
                    .map(|folder| folder.children.len())
                    .unwrap_or(0);
                return (origin.parent, origin.position.min(len));
            }
        }

        let root = self.workspace.root;
        let position = self
            .workspace
            .folder(root)
            .map(|folder| folder.children.len())
            .unwrap_or(0);
        (root, position)
    }

    fn is_valid_scheme_restore_folder(&self, folder_id: FolderId) -> bool {
        self.workspace.folder(folder_id).is_some()
    }

    fn is_valid_folder_restore_parent(&self, folder_id: FolderId) -> bool {
        self.workspace.folder(folder_id).is_some()
            && !self
                .workspace
                .is_node_in_deleted_folder_subtree(NodeRef::Folder(folder_id))
    }
}
