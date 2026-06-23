use knotq_model::{FolderId, ItemId, NodeRef, SchemeId};

use super::super::{KnotQApp, View};

impl KnotQApp {
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
}
