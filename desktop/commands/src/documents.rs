use knotq_model::SchemeId;

use crate::Command;

/// The synced documents a command's effects live in: the workspace index
/// document, and the content document of each scheme it writes.
///
/// This is the single mapping every client uses to decide what to re-encode
/// after applying a command. It lives next to [`Command`] so a new variant has
/// to be classified here in the same change — the match is exhaustive — rather
/// than in one copy per platform that can drift.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandDocuments {
    pub workspace: bool,
    pub schemes: Vec<SchemeId>,
}

impl CommandDocuments {
    fn touch_scheme(&mut self, scheme: SchemeId) {
        if !self.schemes.contains(&scheme) {
            self.schemes.push(scheme);
        }
    }
}

impl Command {
    pub fn crdt_documents(&self) -> CommandDocuments {
        let mut documents = CommandDocuments::default();
        collect(self, &mut documents);
        documents
    }
}

fn collect(command: &Command, out: &mut CommandDocuments) {
    match command {
        Command::CreateFolder { .. }
        | Command::RestoreFolder { .. }
        | Command::RenameFolder { .. }
        | Command::SetFolderExpanded { .. }
        | Command::DeleteFolder { .. }
        | Command::PermanentlyDeleteFolder { .. }
        | Command::CreateScheme { .. }
        | Command::RenameScheme { .. }
        | Command::SetSchemeColor { .. }
        | Command::SetSchemeGsync { .. }
        | Command::SetSchemeSource { .. }
        | Command::DeleteScheme { .. }
        | Command::PermanentlyDeleteScheme { .. }
        | Command::MoveNode { .. } => {
            out.workspace = true;
        }
        Command::RestoreScheme { scheme, .. } | Command::RestoreDeletedScheme { scheme, .. } => {
            out.workspace = true;
            out.touch_scheme(scheme.id);
        }
        Command::RestoreDeletedFolder { schemes, .. } => {
            out.workspace = true;
            for scheme in schemes {
                out.touch_scheme(scheme.id);
            }
        }
        Command::EnsureDailyQueue { date } => {
            // The day's binding lives in the index; its rows (the placeholder)
            // in the day's own content document.
            out.workspace = true;
            out.touch_scheme(knotq_model::daily_queue_scheme_id(*date));
        }
        Command::InsertItem { scheme, .. }
        | Command::UpdateItemText { scheme, .. }
        | Command::ReplaceItem { scheme, .. }
        | Command::SetItemIndent { scheme, .. }
        | Command::SetItemMarker { scheme, .. }
        | Command::SetItemMarkerFamily { scheme, .. }
        | Command::SetItemDate { scheme, .. }
        | Command::SetItemRecurrence { scheme, .. }
        | Command::SetItemPriority { scheme, .. }
        | Command::SetOccurrenceNotificationOffset { scheme, .. }
        | Command::ToggleOccurrence { scheme, .. }
        | Command::DeleteItem { scheme, .. }
        | Command::ReorderItem { scheme, .. } => {
            out.touch_scheme(*scheme);
        }
        Command::Batch(commands) => {
            for command in commands {
                collect(command, out);
            }
        }
    }
}
