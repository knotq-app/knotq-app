//! The Backups section: the user's way back to a workspace as it was.
//!
//! Restoring never happens from here. The store holds the workspace and the
//! CRDT documents in memory and would write them straight back over the
//! restored files, so this records a request and `bootstrap` carries it out at
//! the next launch, before anything reads the directory. The UI's whole job is
//! to make that two-step shape obvious rather than surprising.

use gpui::Context;
use knotq_storage_json::{cancel_restore, data_dir, list_snapshots, request_restore};

use super::components::{settings_action_row, settings_message, SettingsActionRowArgs};
use crate::app::KnotQApp;
use crate::theme_gpui::Theme as UiTheme;

/// Stable element ids, one per snapshot slot. `settings_action_row` takes a
/// `&'static str`, and a week of snapshots needs no more than this.
const ROW_IDS: [&str; 7] = [
    "snapshot-restore-0",
    "snapshot-restore-1",
    "snapshot-restore-2",
    "snapshot-restore-3",
    "snapshot-restore-4",
    "snapshot-restore-5",
    "snapshot-restore-6",
];

impl KnotQApp {
    pub(super) fn backup_rows(
        &mut self,
        t: UiTheme,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let dir = data_dir();
        let mut rows = Vec::new();

        // A pending restore takes over the section. Showing the list as usual
        // underneath would invite a second click that silently replaces the
        // first choice.
        if let Some(day) = knotq_storage_json::pending_restore(&dir) {
            rows.push(settings_message(
                format!("Restoring the workspace from {day} when KnotQ next starts."),
                false,
                t,
            ));
            rows.push(settings_action_row(
                SettingsActionRowArgs {
                    id: "snapshot-restore-cancel",
                    title: "Keep the current workspace".to_string(),
                    detail: "Cancels the restore above.".to_string(),
                    button_label: "Cancel",
                    primary: false,
                },
                t,
                cx,
                |this, cx| {
                    cancel_restore(&data_dir());
                    this.reveal_or_notify(None, cx);
                },
            ));
            return rows;
        }

        let snapshots = list_snapshots(&dir);
        if snapshots.is_empty() {
            rows.push(settings_message(
                "No backups yet. KnotQ saves one each day it is opened.".to_string(),
                false,
                t,
            ));
        }
        for (slot, snapshot) in snapshots.iter().take(ROW_IDS.len()).enumerate() {
            let day = snapshot.day.clone();
            rows.push(settings_action_row(
                SettingsActionRowArgs {
                    id: ROW_IDS[slot],
                    title: day.clone(),
                    detail: if slot == 0 {
                        "Most recent — taken when KnotQ started.".to_string()
                    } else {
                        "Restores on the next launch.".to_string()
                    },
                    button_label: "Restore",
                    primary: false,
                },
                t,
                cx,
                move |this, cx| {
                    let dir = data_dir();
                    let Some(snapshot) = list_snapshots(&dir)
                        .into_iter()
                        .find(|candidate| candidate.day == day)
                    else {
                        return;
                    };
                    if let Err(error) = request_restore(&dir, &snapshot) {
                        eprintln!("could not record a restore: {error:#}");
                    }
                    this.reveal_or_notify(None, cx);
                },
            ));
        }

        rows.push(settings_action_row(
            SettingsActionRowArgs {
                id: "snapshot-reveal",
                title: "Backup folder".to_string(),
                detail: dir.join("snapshots").display().to_string(),
                button_label: "Open",
                primary: false,
            },
            t,
            cx,
            |this, cx| {
                let folder = data_dir().join("snapshots");
                this.reveal_or_notify(Some(folder), cx);
            },
        ));
        rows
    }

    /// Open `folder` in the platform file manager, and repaint either way.
    ///
    /// A backup the user cannot open by hand is not much of a backup — the
    /// copies are deliberately plain files so a support call can walk someone
    /// through them.
    fn reveal_or_notify(&mut self, folder: Option<std::path::PathBuf>, cx: &mut Context<Self>) {
        if let Some(folder) = folder {
            let command = if cfg!(target_os = "macos") {
                "open"
            } else if cfg!(target_os = "windows") {
                "explorer"
            } else {
                "xdg-open"
            };
            if let Err(error) = std::process::Command::new(command).arg(&folder).spawn() {
                eprintln!("could not open {}: {error}", folder.display());
            }
        }
        cx.notify();
    }
}
