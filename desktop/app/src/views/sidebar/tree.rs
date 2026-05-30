use super::*;

impl KnotQApp {
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
                        fid,
                        folder_id,
                        position,
                        depth,
                        t,
                        context_menu_open,
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
