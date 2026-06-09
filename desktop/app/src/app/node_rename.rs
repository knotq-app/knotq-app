use gpui::prelude::*;
use gpui::{Context, Window};
use knotq_commands::{Command, CommandError};
use knotq_model::{FolderId, Item, NodeRef, Scheme};

use super::{KnotQApp, NewNodeKind, RenameNodeState};
use knotq_ui::single_line_editor::{SingleLineEditor, SingleLineEditorEvent};

impl KnotQApp {
    pub(crate) fn new_item_parent_folder(&self) -> FolderId {
        let root = self.workspace.root;
        let Some(scheme_id) = self.selection.scheme_id else {
            return root;
        };
        if self.workspace.is_daily_queue_scheme(scheme_id) {
            return root;
        }
        self.workspace
            .path_to(NodeRef::Scheme(scheme_id))
            .last()
            .copied()
            .filter(|folder_id| self.workspace.folder(*folder_id).is_some())
            .unwrap_or(root)
    }

    pub fn open_new_node_prompt(
        &mut self,
        parent: FolderId,
        kind: NewNodeKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.finish_renaming_node(false, window, cx) {
            return;
        }
        let name = self.unique_new_node_name(parent, kind);

        match kind {
            NewNodeKind::Folder => {
                if parent != self.workspace.root
                    && self
                        .workspace
                        .folder(parent)
                        .is_some_and(|folder| !folder.expanded)
                {
                    self.apply(
                        Command::SetFolderExpanded {
                            id: parent,
                            expanded: true,
                        },
                        cx,
                    );
                }
                let receipt = self.apply(
                    Command::CreateFolder {
                        parent,
                        name,
                        position: None,
                    },
                    cx,
                );
                if let Some(receipt) = receipt {
                    if let Some(id) = created_folder_id_from_inverse(receipt.inverse) {
                        self.start_renaming_node(NodeRef::Folder(id), window, cx);
                    }
                }
            }
            NewNodeKind::Scheme => {
                if parent != self.workspace.root
                    && self
                        .workspace
                        .folder(parent)
                        .is_some_and(|folder| !folder.expanded)
                {
                    self.apply(
                        Command::SetFolderExpanded {
                            id: parent,
                            expanded: true,
                        },
                        cx,
                    );
                }

                let color_index =
                    (self.workspace.schemes.len() as u8) % crate::theme_gpui::PALETTE.len() as u8;
                let mut scheme = Scheme::new(name, color_index);
                scheme.items.push(Item::new(""));
                let receipt = self.apply(
                    Command::RestoreScheme {
                        folder: parent,
                        position: 0,
                        scheme,
                    },
                    cx,
                );
                if let Some(receipt) = receipt {
                    if let Command::DeleteScheme { id } = receipt.inverse {
                        self.open_scheme(id, None);
                        self.start_renaming_node(NodeRef::Scheme(id), window, cx);
                    }
                }
            }
        }
    }

    pub fn cancel_new_node_prompt(&mut self, cx: &mut Context<Self>) {
        if self.rename_node.take().is_some() {
            cx.notify();
        }
    }

    pub fn start_renaming_node(
        &mut self,
        target: NodeRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(original_name) = self.navigator_node_name(target) else {
            return;
        };
        let input = cx.new(|cx| SingleLineEditor::new("Name", original_name.clone(), window, cx));
        let sub = cx.subscribe_in(&input, window, Self::on_rename_node_input_event);
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.rename_node = Some(RenameNodeState {
            target,
            original_name,
            input,
            error: None,
            _subscription: sub,
        });
        cx.notify();
    }

    pub fn start_renaming_current_scheme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(scheme_id) = self.selection.scheme_id {
            self.start_renaming_node(NodeRef::Scheme(scheme_id), window, cx);
        }
    }

    pub(crate) fn navigator_node_name(&self, target: NodeRef) -> Option<String> {
        match target {
            NodeRef::Folder(id) => self.workspace.folder(id).map(|folder| folder.name.clone()),
            NodeRef::Scheme(id) => self.workspace.scheme(id).map(|scheme| scheme.name.clone()),
        }
    }

    pub(crate) fn finish_renaming_node(
        &mut self,
        focus_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(rename) = self.rename_node.take() else {
            return true;
        };
        let name = rename.input.read(cx).value().to_string();
        if name != rename.original_name {
            let command = match rename.target {
                NodeRef::Folder(id) => Command::RenameFolder { id, name },
                NodeRef::Scheme(id) => Command::RenameScheme { id, name },
            };
            match self.apply_result(command, cx) {
                Ok(_) => {}
                Err(err) => {
                    self.keep_rename_error(rename, command_error_message(&err), cx);
                    return false;
                }
            }
        }
        if focus_editor && matches!(rename.target, NodeRef::Scheme(_)) {
            self.focus_current_editor(window, cx);
        }
        cx.notify();
        true
    }

    fn on_rename_node_input_event(
        &mut self,
        _input: &gpui::Entity<SingleLineEditor>,
        event: &SingleLineEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SingleLineEditorEvent::Submit => {
                self.finish_renaming_node(true, window, cx);
            }
            SingleLineEditorEvent::Blur => {
                self.finish_renaming_node(false, window, cx);
            }
            SingleLineEditorEvent::Cancel => {
                let focus_editor = self
                    .rename_node
                    .as_ref()
                    .is_some_and(|rename| matches!(rename.target, NodeRef::Scheme(_)));
                self.rename_node = None;
                if focus_editor {
                    self.focus_current_editor(window, cx);
                }
                cx.notify();
            }
            SingleLineEditorEvent::Change => {
                if let Some(rename) = self.rename_node.as_mut() {
                    if rename.error.is_some() {
                        rename.error = None;
                        cx.notify();
                    }
                }
            }
            SingleLineEditorEvent::Focus => {}
        }
    }

    fn keep_rename_error(
        &mut self,
        mut rename: RenameNodeState,
        error: String,
        cx: &mut Context<Self>,
    ) {
        rename.error = Some(error);
        self.rename_node = Some(rename);
        cx.notify();
    }

    fn unique_new_node_name(&self, parent: FolderId, kind: NewNodeKind) -> String {
        let _ = parent;
        match kind {
            NewNodeKind::Folder => "Untitled Folder",
            NewNodeKind::Scheme => "Untitled",
        }
        .to_string()
    }
}

fn created_folder_id_from_inverse(command: Command) -> Option<FolderId> {
    match command {
        Command::DeleteFolder { id } => Some(id),
        Command::Batch(commands) => commands
            .into_iter()
            .find_map(created_folder_id_from_inverse),
        _ => None,
    }
}

fn command_error_message(err: &CommandError) -> String {
    match err {
        CommandError::DuplicateFolderName { name, .. } => {
            format!("A folder named \"{name}\" already exists in this location.")
        }
        CommandError::DuplicateSchemeName { name, .. } => {
            format!("An item named \"{name}\" already exists in this folder.")
        }
        _ => format!("Could not rename: {err}"),
    }
}
