use gpui::{Context, PathPromptOptions};

use super::KnotQApp;

impl KnotQApp {
    /// Export the whole workspace to a folder of Markdown files (one per
    /// scheme, mirroring the sidebar's folder structure) under a
    /// user-chosen destination, picked via a native folder dialog.
    pub(crate) fn export_workspace_to_markdown(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let time_format = self.time_format;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(knotq_l10n::t("export.choose_destination").into()),
        });

        cx.spawn(async move |_weak: gpui::WeakEntity<Self>, cx| {
            let Ok(Ok(Some(mut paths))) = prompt.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let destination = paths.remove(0);
            match knotq_storage_json::export_workspace_to_markdown(
                &workspace,
                &destination,
                time_format,
            ) {
                Ok(created_dir) => {
                    let _ = cx.update(|cx| cx.reveal_path(&created_dir));
                }
                Err(err) => {
                    eprintln!("failed to export workspace to markdown: {err:#}");
                }
            }
        })
        .detach();
    }
}
