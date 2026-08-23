use super::rows::FolderRowArgs;
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavigatorRow {
    DropZone {
        parent: FolderId,
        position: usize,
        depth: usize,
    },
    EmptyFolder {
        folder_id: FolderId,
        depth: usize,
    },
    Folder {
        folder_id: FolderId,
        parent: FolderId,
        position: usize,
        depth: usize,
    },
    Scheme {
        scheme_id: SchemeId,
        parent: FolderId,
        position: usize,
        depth: usize,
    },
}

impl NavigatorRow {
    pub(super) fn height(self) -> f32 {
        match self {
            Self::DropZone { .. } => NAV_DROP_ZONE_HEIGHT,
            Self::EmptyFolder { .. } | Self::Folder { .. } | Self::Scheme { .. } => NAV_ROW_HEIGHT,
        }
    }
}

impl KnotQApp {
    pub(super) fn flatten_navigator_rows(&self) -> Vec<NavigatorRow> {
        let mut rows = Vec::new();
        self.flatten_node_children(self.workspace.root, 0, &mut rows);
        rows
    }

    fn flatten_node_children(
        &self,
        folder_id: FolderId,
        depth: usize,
        rows: &mut Vec<NavigatorRow>,
    ) {
        let Some(folder) = self.workspace.folder(folder_id) else {
            return;
        };

        let visible_children = folder
            .children
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, child)| !self.is_hidden_navigator_node(*child))
            .collect::<Vec<_>>();

        if visible_children.is_empty() && folder_id != self.workspace.root {
            rows.push(NavigatorRow::EmptyFolder { folder_id, depth });
        }

        for (position, child) in visible_children {
            rows.push(NavigatorRow::DropZone {
                parent: folder_id,
                position,
                depth,
            });
            match child {
                NodeRef::Folder(child_id) => {
                    let Some(child_folder) = self.workspace.folder(child_id) else {
                        continue;
                    };
                    rows.push(NavigatorRow::Folder {
                        folder_id: child_id,
                        parent: folder_id,
                        position,
                        depth,
                    });
                    if child_folder.expanded {
                        self.flatten_node_children(child_id, depth + 1, rows);
                    }
                }
                NodeRef::Scheme(scheme_id) => {
                    if self.workspace.scheme(scheme_id).is_some() {
                        rows.push(NavigatorRow::Scheme {
                            scheme_id,
                            parent: folder_id,
                            position,
                            depth,
                        });
                    }
                }
            }
        }

        rows.push(NavigatorRow::DropZone {
            parent: folder_id,
            position: folder.children.len(),
            depth,
        });
    }

    pub(super) fn render_navigator_row(
        &mut self,
        row: NavigatorRow,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme();
        match row {
            NavigatorRow::DropZone {
                parent,
                position,
                depth,
            } => render_drop_insertion_zone(parent, position, depth, t, cx),
            NavigatorRow::EmptyFolder { folder_id, depth } => {
                empty_folder_placeholder(folder_id, depth, t)
            }
            NavigatorRow::Folder {
                folder_id,
                parent,
                position,
                depth,
            } => self
                .render_folder_row(
                    FolderRowArgs {
                        fid: folder_id,
                        parent_folder_id: parent,
                        position,
                        depth,
                        t,
                        context_menu_open: self.sidebar_context_menu.is_some(),
                    },
                    cx,
                )
                .map(|(row, _)| row)
                .unwrap_or_else(|| div().h(px(NAV_ROW_HEIGHT)).into_any_element()),
            NavigatorRow::Scheme {
                scheme_id,
                parent,
                position,
                depth,
            } => self
                .render_scheme_row(
                    scheme_id,
                    parent,
                    position,
                    depth,
                    t,
                    self.selection.view == View::Scheme,
                    self.selection.scheme_id,
                    self.sidebar_context_menu.is_some(),
                    cx,
                )
                .unwrap_or_else(|| div().h(px(NAV_ROW_HEIGHT)).into_any_element()),
        }
    }

    pub(super) fn render_node_children(
        &mut self,
        folder_id: FolderId,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.theme();
        let selected_id = self.selection.scheme_id;
        let is_scheme_view = self.selection.view == View::Scheme;
        let context_menu_open = self.sidebar_context_menu.is_some();
        let mut items: Vec<gpui::AnyElement> = Vec::new();

        let children = self
            .workspace
            .folder(folder_id)
            .map(|f| f.children.clone())
            .unwrap_or_default();
        let visible_children = children
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, child)| !self.is_hidden_navigator_node(*child))
            .collect::<Vec<_>>();

        if visible_children.is_empty() && folder_id != self.workspace.root {
            items.push(empty_folder_placeholder(folder_id, depth, t));
        }

        for (position, child) in visible_children {
            items.push(render_drop_insertion_zone(
                folder_id, position, depth, t, cx,
            ));

            match child {
                NodeRef::Folder(fid) => {
                    if let Some((row, expanded)) = self.render_folder_row(
                        FolderRowArgs {
                            fid,
                            parent_folder_id: folder_id,
                            position,
                            depth,
                            t,
                            context_menu_open,
                        },
                        cx,
                    ) {
                        items.push(row);
                        if expanded {
                            items.push(
                                self.render_node_children(fid, depth + 1, cx)
                                    .into_any_element(),
                            );
                        }
                    }
                }
                NodeRef::Scheme(sid) => {
                    if let Some(row) = self.render_scheme_row(
                        sid,
                        folder_id,
                        position,
                        depth,
                        t,
                        is_scheme_view,
                        selected_id,
                        context_menu_open,
                        cx,
                    ) {
                        items.push(row);
                    }
                }
            }
        }

        items.push(render_drop_insertion_zone(
            folder_id,
            children.len(),
            depth,
            t,
            cx,
        ));

        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(0.0))
            .children(items)
    }

    fn is_hidden_navigator_node(&self, node: NodeRef) -> bool {
        match node {
            NodeRef::Scheme(id) => self.workspace.is_daily_queue_scheme(id),
            NodeRef::Folder(_) => false,
        }
    }
}
