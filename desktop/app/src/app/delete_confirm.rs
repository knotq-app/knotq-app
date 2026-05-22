use gpui::Context;
use knotq_model::{FolderId, NodeRef, SchemeId};

use super::{DeleteConfirmation, KnotQApp};

impl KnotQApp {
    pub fn request_delete_folder(&mut self, folder_id: FolderId, cx: &mut Context<Self>) {
        if folder_id == self.workspace.root {
            return;
        }
        let Some(folder) = self.workspace.folder(folder_id) else {
            return;
        };
        let scheme_count = folder
            .children
            .iter()
            .filter(|child| matches!(child, NodeRef::Scheme(_)))
            .count();
        let message = match scheme_count {
            0 => format!("Delete empty folder \"{}\" from the workspace?", folder.name),
            1 => format!(
                "Move the scheme in \"{}\" to Archive and remove the folder? The scheme's tasks and calendar items can be restored later",
                folder.name
            ),
            count => format!(
                "Move the {count} schemes in \"{}\" to Archive and remove the folder? Their tasks and calendar items can be restored later",
                folder.name
            ),
        };
        self.pending_delete = Some(DeleteConfirmation {
            target: NodeRef::Folder(folder_id),
            title: if scheme_count == 0 {
                "Delete folder".to_string()
            } else {
                "Move folder contents to Archive".to_string()
            },
            message,
            confirm_label: if scheme_count == 0 {
                "Delete".to_string()
            } else {
                "Move to Archive".to_string()
            },
        });
        cx.notify();
    }

    pub fn request_delete_scheme(&mut self, scheme_id: SchemeId, cx: &mut Context<Self>) {
        let Some(scheme) = self.workspace.scheme(scheme_id) else {
            return;
        };
        if self.workspace.is_daily_queue_scheme(scheme_id) {
            return;
        }
        self.pending_delete = Some(DeleteConfirmation {
            target: NodeRef::Scheme(scheme_id),
            title: "Move item to Archive".to_string(),
            message: format!(
                "Move \"{}\" to Archive? Its tasks and calendar items can be restored later",
                scheme.name
            ),
            confirm_label: "Move to Archive".to_string(),
        });
        cx.notify();
    }

    pub fn cancel_delete_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.pending_delete.take().is_some() {
            cx.notify();
        }
    }

    pub fn confirm_pending_delete(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };
        match pending.target {
            NodeRef::Folder(id) => self.delete_folder(id, cx),
            NodeRef::Scheme(id) => self.delete_scheme(id, cx),
        }
        cx.notify();
    }
}
