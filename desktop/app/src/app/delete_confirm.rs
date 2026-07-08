use gpui::Context;
use knotq_l10n::{t, t_count, t_with};
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

    pub(crate) fn request_unlink_google_account(
        &mut self,
        account_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        self.pending_delete = Some(super::DeleteConfirmation {
            target: ConfirmationTarget::GoogleAccount { account_id },
            title: t("modal.unlink_google_title").to_string(),
            message: t_with("modal.unlink_google_message", &[("label", &label)]),
            confirm_label: t("modal.unlink_google_confirm").to_string(),
        });
        cx.notify();
    }

    pub(crate) fn request_empty_archive_confirmation(&mut self, cx: &mut Context<Self>) {
        let count = self.workspace.recently_deleted.len();
        if count == 0 {
            return;
        }

        self.pending_delete = Some(super::DeleteConfirmation {
            target: ConfirmationTarget::EmptyArchive,
            title: t("archive.empty_confirm_title").to_string(),
            message: t_count("archive.delete_confirm_body", count as i64),
            confirm_label: t("archive.empty_confirm_button").to_string(),
        });
        cx.notify();
    }

    pub fn confirm_pending_delete(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };
        match pending.target {
            ConfirmationTarget::EmptyArchive => self.empty_archive(cx),
            ConfirmationTarget::GoogleAccount { account_id } => {
                self.unlink_google_account(account_id, cx)
            }
        }
        cx.notify();
    }
}
