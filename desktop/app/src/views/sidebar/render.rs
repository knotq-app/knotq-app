use super::*;

impl KnotQApp {
    pub fn render_sidebar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.theme();
        let is_union = self.selection.view == View::Union;
        let is_daily_queue = self.selection.view == View::DailyQueue;
        let context_menu_open = self.sidebar_context_menu.is_some();

        let root_id = self.workspace.root;

        div()
            .flex()
            .flex_col()
            .w(px(166.0))
            .h_full()
            .flex_shrink_0()
            .pt(px(10.0))
            .px(px(8.0))
            .pb(px(8.0))
            .bg(token_hsla(t.bg_sidebar))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .rounded(px(13.0))
            .shadow_md()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.open_sidebar_context_menu(
                        SidebarContextTarget::Background,
                        event.position,
                        cx,
                    );
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .mb(px(6.0))
                    .child(special_row(
                        knotq_l10n::t("sidebar.calendar_label"),
                        if t.is_dark {
                            0xffffffff
                        } else {
                            t.text_primary
                        },
                        is_union,
                        t,
                        context_menu_open,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.open_union();
                            this.focus_app_root(window);
                            cx.notify();
                        }),
                    ))
                    .child(special_row(
                        DAILY_QUEUE_TITLE,
                        daily_queue_marker_color(t.is_dark),
                        is_daily_queue,
                        t,
                        context_menu_open,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.open_daily_queue(cx);
                            this.focus_current_editor(window, cx);
                            cx.notify();
                        }),
                    ))
                    .child(self.render_trash_section(cx)),
            )
            .child(
                div()
                    .h(px(1.0))
                    .bg(token_rgba(t.divider))
                    .mx(px(3.0))
                    .mb(px(8.0)),
            )
            .child(
                div()
                    .id("sidebar-tree")
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(self.render_node_children(root_id, 0, cx)),
            )
            .child(self.render_sidebar_footer(cx))
    }

    fn render_sidebar_footer(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme();
        div()
            .px(px(2.0))
            .pt(px(4.0))
            .pb(px(2.0))
            .flex()
            .child(footer_button(
                "sidebar-new-menu",
                knotq_l10n::t("sidebar.footer.new"),
                Icon::empty()
                    .path("icons/plus.svg")
                    .with_size(px(11.0))
                    .text_color(token_hsla(t.text_dim))
                    .into_any_element(),
                t,
                cx.listener(move |this, event: &ClickEvent, _window, cx| {
                    let parent = this.new_item_parent_folder();
                    this.open_sidebar_context_menu(
                        SidebarContextTarget::NewMenu { parent },
                        event.position(),
                        cx,
                    );
                }),
            ))
            .into_any_element()
    }
}
