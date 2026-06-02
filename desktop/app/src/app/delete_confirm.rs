use gpui::Context;
use knotq_model::{FolderId, SchemeId};

use super::{ConfirmationTarget, KnotQApp};

impl KnotQApp {
    pub fn request_delete_folder(&mut self, folder_id: FolderId, cx: &mut Context<Self>) {
        if folder_id == self.workspace.root {
            return;
        }
        if self.workspace.folder(folder_id).is_none() {
            return;
        }
        self.delete_folder(folder_id, cx);
        cx.notify();
    }

    pub fn request_delete_scheme(&mut self, scheme_id: SchemeId, cx: &mut Context<Self>) {
        if self.workspace.scheme(scheme_id).is_none() {
            return;
        }
        if self.workspace.is_daily_queue_scheme(scheme_id) {
            return;
        }
        self.delete_scheme(scheme_id, cx);
        cx.notify();
    }

    pub fn cancel_delete_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.pending_delete.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn request_forget_google_account(
        &mut self,
        account_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        self.pending_delete = Some(super::DeleteConfirmation {
            target: ConfirmationTarget::GoogleAccount { account_id },
            title: "Forget Google account?".to_string(),
            message: format!(
                "This removes local Google Calendar access for {label}. Synced Google calendar schemes remain, but they will show as offline until you sign in locally again."
            ),
            confirm_label: "Forget".to_string(),
        });
        cx.notify();
    }

    pub fn confirm_pending_delete(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };
        match pending.target {
            ConfirmationTarget::GoogleAccount { account_id } => {
                self.forget_google_account(account_id, cx)
            }
        }
        cx.notify();
    }
}
