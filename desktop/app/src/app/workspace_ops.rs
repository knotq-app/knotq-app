use gpui::Context;
use knotq_commands::Command;
use knotq_model::{CalendarProvider, FolderId, ItemId, NodeRef, Scheme, SchemeId, SchemeSource};

use super::{KnotQApp, View, DAILY_QUEUE_TITLE};

impl KnotQApp {
    pub fn current_scheme(&self) -> Option<&Scheme> {
        let id = self.selection.scheme_id?;
        self.workspace.scheme(id)
    }

    pub(crate) fn scheme_display_name(&self, scheme: &Scheme) -> String {
        if self.workspace.is_daily_queue_scheme(scheme.id) {
            DAILY_QUEUE_TITLE.to_string()
        } else {
            scheme.name.clone()
        }
    }

    pub(crate) fn imported_calendar_account_label(&self, scheme: &Scheme) -> Option<String> {
        let SchemeSource::ImportedCalendar(source) = &scheme.source else {
            return None;
        };
        match source.provider {
            CalendarProvider::Google => self
                .settings
                .google_accounts
                .iter()
                .find(|account| account.account_id == source.account_id)
                .and_then(|account| account.email.clone())
                .or_else(|| Some(source.account_id.clone())),
            CalendarProvider::Apple | CalendarProvider::Ics => Some(source.account_id.clone()),
        }
        .filter(|label| !label.trim().is_empty())
    }

    pub fn toggle_folder(&mut self, id: FolderId, cx: &mut Context<Self>) {
        let cur = self
            .workspace
            .folder(id)
            .map(|f| f.expanded)
            .unwrap_or(true);
        self.apply(Command::SetFolderExpanded { id, expanded: !cur }, cx);
    }

    pub fn toggle_trash(&mut self, cx: &mut Context<Self>) {
        self.trash_expanded = !self.trash_expanded;
        cx.notify();
    }

    pub fn delete_folder(&mut self, folder_id: FolderId, cx: &mut Context<Self>) {
        if folder_id == self.workspace.root {
            return;
        }
        let had_schemes = self.workspace.folder(folder_id).is_some_and(|folder| {
            folder
                .children
                .iter()
                .any(|child| matches!(child, NodeRef::Scheme(_)))
        });
        if self.workspace.folder(folder_id).is_none() {
            return;
        }
        if self
            .apply(Command::DeleteFolder { id: folder_id }, cx)
            .is_some()
        {
            self.trash_expanded |= had_schemes;
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

    pub fn empty_archive(&mut self, cx: &mut Context<Self>) {
        let deleted = self.workspace.recently_deleted.clone();
        if deleted.is_empty() {
            return;
        }
        let commands = deleted
            .iter()
            .copied()
            .map(|id| Command::PermanentlyDeleteScheme { id })
            .collect::<Vec<_>>();
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

    /// Reconcile UI state after a workspace mutation: close popovers for
    /// deleted items, update selections when the active scheme disappears, etc.
    pub(crate) fn reconcile_workspace_ui_state(&mut self) {
        if self
            .rename_node
            .as_ref()
            .is_some_and(|rename| !self.navigator_node_exists(rename.target))
        {
            self.rename_node = None;
        }

        if self
            .scheme_editor
            .as_ref()
            .is_some_and(|(id, _)| self.workspace.scheme(*id).is_none())
        {
            self.scheme_editor = None;
            self._editor_subscription = None;
        }
        let stale_sessions = self
            .scheme_sessions
            .keys()
            .copied()
            .filter(|scheme_id| self.workspace.scheme(*scheme_id).is_none())
            .collect::<Vec<_>>();
        for scheme_id in stale_sessions {
            self.scheme_sessions.remove(&scheme_id);
        }

        if self
            .event_popup
            .as_ref()
            .is_some_and(|popup| !self.scheme_item_exists(popup.scheme_id, popup.item_id))
        {
            self.event_popup = None;
            self.event_popup_title_subscription = None;
        }

        if self
            .date_popover
            .as_ref()
            .is_some_and(|popup| !self.scheme_item_exists(popup.scheme_id, popup.item_id))
        {
            self.date_popover = None;
        }
        if self
            .repeat_popover
            .as_ref()
            .is_some_and(|popup| !self.scheme_item_exists(popup.scheme_id, popup.item_id))
        {
            self.repeat_popover = None;
            self.recurrence_undo_group = None;
        }

        if let Some(scheme_id) = self.selection.scheme_id {
            if self.workspace.scheme(scheme_id).is_none() {
                self.selection.focused_item_id = None;
                if matches!(self.selection.view, View::Scheme) {
                    if let Some(next_id) = self.first_visible_scheme_id() {
                        self.open_scheme(next_id, None);
                    } else {
                        self.open_union();
                        self.selection.scheme_id = None;
                    }
                } else {
                    self.selection.scheme_id = None;
                }
            } else if self
                .selection
                .focused_item_id
                .is_some_and(|item_id| !self.scheme_item_exists(scheme_id, item_id))
            {
                self.selection.focused_item_id = None;
            }
        }
    }

    pub(crate) fn navigator_node_exists(&self, target: NodeRef) -> bool {
        match target {
            NodeRef::Folder(id) => self.workspace.folder(id).is_some(),
            NodeRef::Scheme(id) => self.workspace.scheme(id).is_some(),
        }
    }

    pub(crate) fn scheme_item_exists(&self, scheme_id: SchemeId, item_id: ItemId) -> bool {
        self.workspace
            .scheme(scheme_id)
            .is_some_and(|scheme| scheme.items.iter().any(|item| item.id == item_id))
    }

    pub(crate) fn first_visible_scheme_id(&self) -> Option<SchemeId> {
        self.first_visible_scheme_in_folder(self.workspace.root)
    }

    fn first_visible_scheme_in_folder(&self, folder_id: FolderId) -> Option<SchemeId> {
        let folder = self.workspace.folder(folder_id)?;
        for child in &folder.children {
            match *child {
                NodeRef::Scheme(id) => {
                    if self.workspace.scheme(id).is_some()
                        && !self.workspace.is_daily_queue_scheme(id)
                    {
                        return Some(id);
                    }
                }
                NodeRef::Folder(id) => {
                    if let Some(found) = self.first_visible_scheme_in_folder(id) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    pub(crate) fn first_visible_scheme_id_except(&self, excluded: SchemeId) -> Option<SchemeId> {
        self.first_visible_scheme_in_folder_except(self.workspace.root, excluded)
    }

    fn first_visible_scheme_in_folder_except(
        &self,
        folder_id: FolderId,
        excluded: SchemeId,
    ) -> Option<SchemeId> {
        let folder = self.workspace.folder(folder_id)?;
        for child in &folder.children {
            match *child {
                NodeRef::Scheme(id) => {
                    if id != excluded
                        && self.workspace.scheme(id).is_some()
                        && !self.workspace.is_daily_queue_scheme(id)
                    {
                        return Some(id);
                    }
                }
                NodeRef::Folder(id) => {
                    if let Some(found) = self.first_visible_scheme_in_folder_except(id, excluded) {
                        return Some(found);
                    }
                }
            }
        }
        None
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

    pub(crate) fn close_popovers_for_scheme(&mut self, scheme_id: SchemeId) {
        if self
            .event_popup
            .as_ref()
            .is_some_and(|p| p.scheme_id == scheme_id)
        {
            self.event_popup = None;
        }
        if self
            .date_popover
            .as_ref()
            .is_some_and(|p| p.scheme_id == scheme_id)
        {
            self.date_popover = None;
        }
        if self
            .repeat_popover
            .as_ref()
            .is_some_and(|p| p.scheme_id == scheme_id)
        {
            self.repeat_popover = None;
            self.recurrence_undo_group = None;
        }
    }

    fn is_valid_scheme_restore_folder(&self, folder_id: FolderId) -> bool {
        folder_id == self.workspace.root
            || self
                .workspace
                .folder(folder_id)
                .is_some_and(|folder| folder.parent == Some(self.workspace.root))
    }
}
