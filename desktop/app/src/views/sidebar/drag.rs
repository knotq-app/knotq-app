use super::*;

#[derive(Clone)]
pub(super) struct NavigatorDragPreview {
    pub(super) info: NavigatorDragInfo,
}

impl Render for NavigatorDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.info.theme;
        let chip = self
            .info
            .color_index
            .map(|index| scheme_square_color(index, t.is_dark))
            .unwrap_or_else(|| token_rgba(t.text_dim));

        div()
            .w(px(178.0))
            .h(px(29.0))
            .rounded(px(7.0))
            .opacity(0.72)
            .bg(token_rgba(t.drag_preview_bg))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .shadow_md()
            .flex()
            .items_center()
            .gap(px(7.0))
            .px(px(9.0))
            .child(match self.info.kind {
                NavigatorNodeKind::Folder => Icon::empty()
                    .path(ZED_FOLDER_ICON)
                    .xsmall()
                    .text_color(token_hsla(t.text_dim))
                    .into_any_element(),
                NavigatorNodeKind::Scheme => Icon::new(IconName::File)
                    .xsmall()
                    .text_color(token_hsla(t.text_dim))
                    .into_any_element(),
            })
            .child(
                div()
                    .w(px(9.0))
                    .h(px(9.0))
                    .rounded(px(2.0))
                    .flex_shrink_0()
                    .bg(chip),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(token_hsla(t.text_highlight))
                    .child(self.info.label.clone()),
            )
    }
}

impl KnotQApp {
    pub(super) fn navigator_node_position(&self, node: NodeRef) -> Option<(FolderId, usize)> {
        self.workspace
            .folders
            .iter()
            .find_map(|(folder_id, folder)| {
                folder
                    .children
                    .iter()
                    .position(|child| *child == node)
                    .map(|position| (*folder_id, position))
            })
    }

    pub(super) fn can_drop_navigator_node(
        &self,
        drag: &NavigatorDragInfo,
        new_parent: FolderId,
        position: usize,
    ) -> bool {
        if !navigator_drop_target_accepts(drag, new_parent, position) {
            return false;
        }
        let Some(parent_folder) = self.workspace.folder(new_parent) else {
            return false;
        };
        if position > parent_folder.children.len() {
            return false;
        }

        let Some((source_parent, source_position)) = self.navigator_node_position(drag.node) else {
            return false;
        };
        if source_parent == new_parent
            && (position == source_position || position == source_position + 1)
        {
            return false;
        }

        match drag.node {
            NodeRef::Folder(id) => {
                id != self.workspace.root
                    && self.workspace.folder(id).is_some()
                    && new_parent == self.workspace.root
            }
            NodeRef::Scheme(id) => {
                self.workspace.scheme(id).is_some()
                    && (new_parent == self.workspace.root
                        || self
                            .workspace
                            .folder(new_parent)
                            .is_some_and(|folder| folder.parent == Some(self.workspace.root)))
            }
        }
    }

    pub(super) fn drop_navigator_node(
        &mut self,
        drag: &NavigatorDragInfo,
        new_parent: FolderId,
        position: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.can_drop_navigator_node(drag, new_parent, position) {
            return;
        }

        let should_expand = new_parent != self.workspace.root
            && self
                .workspace
                .folder(new_parent)
                .is_some_and(|folder| !folder.expanded);
        let move_cmd = Command::MoveNode {
            node: drag.node,
            new_parent,
            position,
        };
        let cmd = if should_expand {
            Command::Batch(vec![
                move_cmd,
                Command::SetFolderExpanded {
                    id: new_parent,
                    expanded: true,
                },
            ])
        } else {
            move_cmd
        };
        self.apply(cmd, cx);
    }
}

pub(super) fn render_drop_insertion_zone(
    parent: FolderId,
    position: usize,
    depth: usize,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let indent = 8.0 + depth as f32 * 9.0;
    let line_color = token_rgba(t.caret_color);
    div()
        .id(SharedString::from(format!(
            "nav-drop-{}-{}",
            parent, position
        )))
        .relative()
        .h(px(NAV_DROP_ZONE_HEIGHT))
        .w_full()
        .min_w_0()
        .mx(px(3.0))
        .opacity(0.0)
        .can_drop(move |dragged, _w, _cx| {
            dragged
                .downcast_ref::<NavigatorDragInfo>()
                .is_some_and(|drag| navigator_drop_target_accepts(drag, parent, position))
        })
        .drag_over::<NavigatorDragInfo>(move |s, drag, _w, _cx| {
            if navigator_drop_target_accepts(drag, parent, position) {
                s.opacity(1.0)
            } else {
                s
            }
        })
        .on_drop(
            cx.listener(move |this, drag: &NavigatorDragInfo, _window, cx| {
                this.drop_navigator_node(drag, parent, position, cx);
            }),
        )
        .child(
            div()
                .absolute()
                .left(px(indent))
                .top(px(0.0))
                .w(px(NAV_DROP_ZONE_HEIGHT))
                .h(px(NAV_DROP_ZONE_HEIGHT))
                .rounded_full()
                .bg(line_color),
        )
        .child(
            div()
                .absolute()
                .left(px(indent + NAV_DROP_ZONE_HEIGHT - 1.0))
                .right(px(0.0))
                .top(px((NAV_DROP_ZONE_HEIGHT - 1.0) / 2.0))
                .h(px(1.5))
                .rounded_full()
                .bg(line_color),
        )
        .into_any_element()
}

pub(super) fn render_scheme_drop_indicator(
    parent: FolderId,
    position: usize,
    depth: usize,
    group: SharedString,
    t: Theme,
) -> gpui::AnyElement {
    let indent = 8.0 + depth as f32 * 9.0;
    let line_color = token_rgba(t.caret_color);
    div()
        .id(SharedString::from(format!(
            "nav-scheme-drop-{}-{}",
            parent, position
        )))
        .group(SharedString::from(format!(
            "nav-scheme-drop-hitbox-{}-{}",
            parent, position
        )))
        .invisible()
        .absolute()
        .left(px(3.0))
        .right(px(3.0))
        .top(px(-NAV_DROP_ZONE_HEIGHT))
        .h(px(NAV_DROP_ZONE_HEIGHT))
        .can_drop(move |dragged, _w, _cx| {
            dragged
                .downcast_ref::<NavigatorDragInfo>()
                .is_some_and(|drag| navigator_drop_target_accepts(drag, parent, position))
        })
        .group_drag_over::<NavigatorDragInfo>(group, |s| s.visible())
        .child(
            div()
                .absolute()
                .left(px(indent))
                .top(px(0.0))
                .w(px(NAV_DROP_ZONE_HEIGHT))
                .h(px(NAV_DROP_ZONE_HEIGHT))
                .rounded_full()
                .bg(line_color),
        )
        .child(
            div()
                .absolute()
                .left(px(indent + NAV_DROP_ZONE_HEIGHT - 1.0))
                .right(px(0.0))
                .top(px((NAV_DROP_ZONE_HEIGHT - 1.0) / 2.0))
                .h(px(1.5))
                .rounded_full()
                .bg(line_color),
        )
        .into_any_element()
}

pub(super) fn navigator_drop_target_accepts(
    drag: &NavigatorDragInfo,
    new_parent: FolderId,
    position: usize,
) -> bool {
    if matches!(drag.node, NodeRef::Folder(id) if id == new_parent) {
        return false;
    }
    if drag.kind == NavigatorNodeKind::Folder && new_parent != drag.root {
        return false;
    }
    if drag.source_parent == new_parent
        && (position == drag.source_position || position == drag.source_position + 1)
    {
        return false;
    }
    true
}
