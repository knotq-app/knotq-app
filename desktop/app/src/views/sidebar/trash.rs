use super::*;

impl KnotQApp {
    pub(super) fn render_trash_section(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme();
        let context_menu_open = self.sidebar_context_menu.is_some();
        let deleted = self
            .workspace
            .iter_deleted_schemes()
            .map(|scheme| (scheme.id, scheme.name.clone(), scheme.color_index))
            .collect::<Vec<_>>();

        let header = trash_header_row(
            self.trash_expanded,
            t,
            context_menu_open,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_trash(cx);
            }),
            cx,
        );

        let mut rows = Vec::with_capacity(deleted.len() + 1);
        if self.trash_expanded {
            if deleted.is_empty() {
                rows.push(empty_trash_placeholder(t));
            } else {
                for (idx, (scheme_id, name, color_index)) in deleted.into_iter().enumerate() {
                    rows.push(trash_scheme_row(
                        idx,
                        scheme_id,
                        name,
                        color_index,
                        t,
                        context_menu_open,
                        cx,
                    ));
                }
            }
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .border_1()
            .border_color(token_rgba(0x00000000))
            .drag_over::<NavigatorDragInfo>(move |s, drag, _w, _cx| {
                if matches!(drag.source, NavigatorDragSource::Active { .. }) {
                    s.border_color(token_rgba(t.border_overlay))
                } else {
                    s
                }
            })
            .can_drop(move |dragged, _w, _cx| {
                dragged
                    .downcast_ref::<NavigatorDragInfo>()
                    .is_some_and(|drag| matches!(drag.source, NavigatorDragSource::Active { .. }))
            })
            .on_drop(
                cx.listener(move |this, drag: &NavigatorDragInfo, _window, cx| {
                    this.archive_navigator_node(drag, cx);
                }),
            )
            .child(header)
            .when(self.trash_expanded, |s| {
                s.child(
                    div()
                        .id("archive-scroll")
                        .max_h(px(NAV_ROW_HEIGHT * ARCHIVE_MAX_VISIBLE_ROWS))
                        .overflow_y_scroll()
                        .children(rows),
                )
            })
            .into_any_element()
    }
}

fn trash_header_row(
    expanded: bool,
    t: Theme,
    context_menu_open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id("sidebar-trash")
        .flex()
        .items_center()
        .gap(px(NAV_ICON_GAP))
        .w_full()
        .h(px(NAV_ROW_HEIGHT))
        .pl(px(NAV_ROW_INDENT_BASE))
        .pr(px(4.0))
        .rounded(px(3.0))
        .cursor_pointer()
        .when(!context_menu_open, move |s| {
            s.hover(move |h| h.bg(token_rgba(t.row_hover)))
        })
        .on_click(on_click)
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
                this.open_sidebar_context_menu(SidebarContextTarget::Archive, event.position, cx);
            }),
        )
        .child(nav_icon_slot(
            Icon::empty()
                .path(DELETE_ICON)
                .with_size(px(11.5))
                .text_color(token_hsla(if expanded {
                    t.text_primary
                } else {
                    t.text_dim
                }))
                .into_any_element(),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .whitespace_nowrap()
                .text_size(px(SIDEBAR_TEXT_SIZE))
                .line_height(px(SIDEBAR_LINE_HEIGHT))
                .font_family(FONT_UI)
                .text_color(token_hsla(t.text_primary))
                .child("Archive"),
        )
        .into_any_element()
}

fn trash_scheme_row(
    idx: usize,
    scheme_id: SchemeId,
    name: String,
    color_index: u8,
    t: Theme,
    context_menu_open: bool,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let square = scheme_square_color(color_index, t.is_dark);
    let drag_info = NavigatorDragInfo {
        node: NodeRef::Scheme(scheme_id),
        kind: NavigatorNodeKind::Scheme,
        source: NavigatorDragSource::Archive,
        label: name.clone(),
        color_index: Some(color_index),
        theme: t,
    };
    div()
        .id(("trash-scheme", idx))
        .flex()
        .items_center()
        .gap(px(NAV_ICON_GAP))
        .w_full()
        .min_w_0()
        .h(px(NAV_ROW_HEIGHT))
        .pl(px(NAV_ROW_INDENT_BASE + 9.0))
        .pr(px(4.0))
        .rounded(px(5.0))
        .when(!context_menu_open, move |s| {
            s.hover(move |h| h.bg(token_rgba(t.row_hover)))
        })
        .on_drag(drag_info, |drag, _position: Point<Pixels>, _w, cx| {
            cx.stop_propagation();
            cx.new(|_| NavigatorDragPreview { info: drag.clone() })
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
                this.open_sidebar_context_menu(
                    SidebarContextTarget::DeletedScheme { scheme_id },
                    event.position,
                    cx,
                );
            }),
        )
        .child(nav_icon_slot(
            div()
                .w(px(SCHEME_SQUARE_SIZE))
                .h(px(SCHEME_SQUARE_SIZE))
                .rounded(px(2.0))
                .bg(square)
                .into_any_element(),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .whitespace_nowrap()
                .text_size(px(SIDEBAR_TEXT_SIZE))
                .line_height(px(SIDEBAR_LINE_HEIGHT))
                .font_family(FONT_UI)
                .text_color(token_hsla(t.text_muted))
                .overflow_hidden()
                .child(name),
        )
        .into_any_element()
}

fn empty_trash_placeholder(t: Theme) -> gpui::AnyElement {
    let indent = NAV_ROW_INDENT_BASE + 9.0 + NAV_ICON_SLOT + NAV_ICON_GAP;
    div()
        .id("empty-trash-row")
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(px(NAV_ROW_HEIGHT))
        .pl(px(indent))
        .pr(px(4.0))
        .rounded(px(5.0))
        .text_size(px(12.0))
        .text_color(token_hsla(t.text_muted))
        .child(div().flex_1().min_w_0().child("No items"))
        .into_any_element()
}
