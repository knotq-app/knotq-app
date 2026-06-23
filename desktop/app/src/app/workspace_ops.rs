use gpui::Context;
use knotq_commands::Command;
use knotq_model::{FolderId, Scheme};

use super::{KnotQApp, DAILY_QUEUE_TITLE};

mod google;
mod reconcile;
mod trash;

pub(crate) use google::{
    emails_match, google_account_has_local_credentials, google_account_matches_calendar_source,
    google_calendar_source_target_label,
};

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
}
