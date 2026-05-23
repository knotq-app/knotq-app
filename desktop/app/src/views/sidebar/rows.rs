use super::*;

impl KnotQApp {
    pub(super) fn render_folder_row(
        &mut self,
        fid: FolderId,
        position: usize,
        depth: usize,
        t: Theme,
        context_menu_open: bool,
        cx: &mut Context<Self>,
    ) -> Option<(gpui::AnyElement, bool)> {
        let folder = self.workspace.folder(fid)?.clone();
        if folder.id == self.workspace.root {
            return None;
        }
        let pl_val = NAV_ROW_INDENT_BASE + depth as f32 * 9.0;
        let folder_id = folder.id;
        let folder_name = folder.name.clone();
        let folder_expanded = folder.expanded;
        let folder_group = SharedString::from(format!("folder-row-{}", folder.id));
        let rename = self
            .rename_node
            .as_ref()
            .filter(|rename| rename.target == NodeRef::Folder(folder.id))
            .map(|rename| (rename.input.clone(), rename.error.clone()));
        let has_rename_error = rename.as_ref().is_some_and(|(_, error)| error.is_some());
        let folder_drop_position = folder.children.len();
        let drag_info = NavigatorDragInfo {
            node: NodeRef::Folder(folder.id),
            kind: NavigatorNodeKind::Folder,
            source: NavigatorDragSource::Active {
                parent: self.workspace.root,
                position,
            },
            root: self.workspace.root,
            label: folder.name.clone(),
            color_index: None,
            theme: t,
        };
        let row = div()
            .id(SharedString::from(format!("folder-{}", folder.id)))
            .group(folder_group.clone())
            .relative()
            .flex()
            .items_center()
            .gap(px(NAV_ICON_GAP))
            .w_full()
            .min_w_0()
            .min_h(px(NAV_ROW_HEIGHT))
            .when(!has_rename_error, |s| s.h(px(NAV_ROW_HEIGHT)))
            .when(has_rename_error, |s| s.items_start().py(px(3.0)))
            .pl(px(pl_val))
            .pr(px(4.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(token_rgba(0x00000000))
            .when(!context_menu_open, move |s| {
                s.hover(move |h| h.bg(token_rgba(t.row_hover)))
            })
            .drag_over::<NavigatorDragInfo>(move |s, drag, _w, _cx| {
                if navigator_drop_target_accepts(drag, folder_id, folder_drop_position) {
                    s.bg(token_rgba(t.row_hover_strong))
                        .border_color(token_rgba(t.border_overlay))
                } else {
                    s
                }
            })
            .can_drop(move |dragged, _w, _cx| {
                dragged
                    .downcast_ref::<NavigatorDragInfo>()
                    .is_some_and(|drag| {
                        navigator_drop_target_accepts(drag, folder_id, folder_drop_position)
                    })
            })
            .on_drop(
                cx.listener(move |this, drag: &NavigatorDragInfo, _window, cx| {
                    this.drop_navigator_node(drag, folder_id, folder_drop_position, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.open_sidebar_context_menu(
                        SidebarContextTarget::Folder(folder_id),
                        event.position,
                        cx,
                    );
                }),
            )
            .child({
                let id = folder.id;
                let base = div()
                    .id(SharedString::from(format!("folder-main-{}", folder.id)))
                    .flex_1()
                    .min_w_0()
                    .when(!has_rename_error, |s| s.h_full())
                    .flex()
                    .items_center()
                    .when(has_rename_error, |s| s.items_start())
                    .gap(px(NAV_ICON_GAP))
                    .child(nav_icon_slot(
                        zed_folder_icon(folder_expanded, t).into_any_element(),
                    ));
                if let Some((input, error)) = rename {
                    base.child(inline_rename_input(input, error, t))
                        .into_any_element()
                } else {
                    base.cursor_pointer()
                        .on_drag(drag_info, |drag, _position: Point<Pixels>, _w, cx| {
                            cx.stop_propagation();
                            cx.new(|_| NavigatorDragPreview { info: drag.clone() })
                        })
                        .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                            this.toggle_folder(id, cx);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .whitespace_nowrap()
                                .text_size(px(SIDEBAR_TEXT_SIZE))
                                .line_height(px(SIDEBAR_LINE_HEIGHT))
                                .font_family(FONT_UI)
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(token_hsla(t.text_primary))
                                .overflow_hidden()
                                .child(folder_name),
                        )
                        .into_any_element()
                }
            })
            .into_any_element();
        Some((row, folder_expanded))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_scheme_row(
        &mut self,
        sid: SchemeId,
        folder_id: FolderId,
        position: usize,
        depth: usize,
        t: Theme,
        is_scheme_view: bool,
        selected_id: Option<SchemeId>,
        context_menu_open: bool,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let scheme = self.workspace.scheme(sid)?.clone();
        let is_sel = is_scheme_view && selected_id == Some(sid);
        let pl_s = NAV_ROW_INDENT_BASE + depth as f32 * 9.0;
        let square = scheme_square_color(scheme.color_index, t.is_dark);
        let scheme_group = SharedString::from(format!("scheme-row-{}", scheme.id));
        let rename = self
            .rename_node
            .as_ref()
            .filter(|rename| rename.target == NodeRef::Scheme(scheme.id))
            .map(|rename| (rename.input.clone(), rename.error.clone()));
        let is_renaming = rename.is_some();
        let has_rename_error = rename.as_ref().is_some_and(|(_, error)| error.is_some());
        let drag_info = NavigatorDragInfo {
            node: NodeRef::Scheme(scheme.id),
            kind: NavigatorNodeKind::Scheme,
            source: NavigatorDragSource::Active {
                parent: folder_id,
                position,
            },
            root: self.workspace.root,
            label: self.scheme_display_name(&scheme),
            color_index: Some(scheme.color_index),
            theme: t,
        };
        let drop_parent = folder_id;
        let drop_position = position;
        let scheme_id = scheme.id;
        let scheme_name = self.scheme_display_name(&scheme);

        Some(
            div()
                .id(SharedString::from(format!("scheme-{}", scheme.id)))
                .group(scheme_group.clone())
                .relative()
                .flex()
                .items_center()
                .gap(px(NAV_ICON_GAP))
                .w_full()
                .min_w_0()
                .min_h(px(NAV_ROW_HEIGHT))
                .when(!has_rename_error, |s| s.h(px(NAV_ROW_HEIGHT)))
                .when(has_rename_error, |s| s.items_start().py(px(3.0)))
                .pl(px(pl_s))
                .pr(px(4.0))
                .rounded(px(5.0))
                .border_1()
                .border_color(token_rgba(if is_sel {
                    if is_renaming {
                        0x00000000
                    } else {
                        t.border_overlay
                    }
                } else {
                    0x00000000
                }))
                .cursor_pointer()
                .when(is_sel, move |s| s.bg(token_rgba(t.row_selected)))
                .when(!is_sel && !context_menu_open, move |s| {
                    s.hover(move |h| h.bg(token_rgba(t.row_hover)))
                })
                .drag_over::<NavigatorDragInfo>(move |s, _drag, _w, _cx| s)
                .can_drop(move |dragged, _w, _cx| {
                    dragged
                        .downcast_ref::<NavigatorDragInfo>()
                        .is_some_and(|drag| {
                            navigator_drop_target_accepts(drag, drop_parent, drop_position)
                        })
                })
                .on_drop(
                    cx.listener(move |this, drag: &NavigatorDragInfo, _window, cx| {
                        this.drop_navigator_node(drag, drop_parent, drop_position, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.open_sidebar_context_menu(
                            SidebarContextTarget::Scheme { scheme_id },
                            event.position,
                            cx,
                        );
                    }),
                )
                .child({
                    let id = scheme.id;
                    let base = div()
                        .id(SharedString::from(format!("scheme-main-{}", scheme.id)))
                        .flex_1()
                        .min_w_0()
                        .when(!has_rename_error, |s| s.h_full())
                        .flex()
                        .items_center()
                        .when(has_rename_error, |s| s.items_start())
                        .gap(px(NAV_ICON_GAP))
                        .child(nav_icon_slot(
                            div()
                                .w(px(SCHEME_SQUARE_SIZE))
                                .h(px(SCHEME_SQUARE_SIZE))
                                .rounded(px(2.0))
                                .bg(square)
                                .into_any_element(),
                        ));
                    if let Some((input, error)) = rename {
                        base.child(inline_rename_input(input, error, t))
                            .into_any_element()
                    } else {
                        base.cursor_pointer()
                            .on_drag(drag_info, |drag, _position: Point<Pixels>, _w, cx| {
                                cx.stop_propagation();
                                cx.new(|_| NavigatorDragPreview { info: drag.clone() })
                            })
                            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                                this.open_scheme(id, None);
                                this.focus_current_editor(window, cx);
                                let _ = ev;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .whitespace_nowrap()
                                    .text_size(px(SIDEBAR_TEXT_SIZE))
                                    .line_height(px(SIDEBAR_LINE_HEIGHT))
                                    .font_family(FONT_UI)
                                    .font_weight(gpui::FontWeight::NORMAL)
                                    .text_color(token_hsla(t.text_primary))
                                    .overflow_hidden()
                                    .child(scheme_name),
                            )
                            .into_any_element()
                    }
                })
                .child(render_scheme_drop_indicator(
                    drop_parent,
                    drop_position,
                    depth,
                    scheme_group,
                    t,
                ))
                .into_any_element(),
        )
    }
}
