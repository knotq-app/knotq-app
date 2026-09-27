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

        let tree = self.render_sidebar_tree(cx);

        let container = div()
            .flex()
            .flex_col()
            .w(px(self.sidebar_width()))
            .h_full()
            .flex_shrink_0()
            // A full-height sidebar puts a band here instead of padding: the
            // traffic lights sit in it, and so does the sync control.
            .pt(px(if full_height_column() {
                0.0
            } else {
                content_top_inset()
            }))
            .px(px(sidebar_side_padding()))
            .pb(px(8.0))
            // The card's fill goes translucent so the window's blur reads
            // through it.
            .bg(sidebar_surface(t));

        // Finder and Mail run the sidebar to the window's edges and give it no
        // chrome of its own: a border or a shadow would be drawn *inside* the
        // vibrancy and read as a lit seam across it. The classic sidebar keeps
        // being a card floating inside the left panel.
        let container = if full_height_column() {
            container
                .border_r_1()
                .border_color(token_rgba(t.divider_faint))
        } else {
            container
                .border_1()
                .border_color(token_rgba(t.border_overlay))
                .rounded(px(13.0))
                .shadow_md()
        };

        container
            .children(self.render_sidebar_title_band(t, cx))
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
                    .mb(px(group_gap()))
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
                    .mb(px(group_gap())),
            )
            .child(
                div()
                    .id("sidebar-tree")
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .overflow_hidden()
                    .child(tree),
            )
            .child(self.render_sidebar_footer(cx))
    }

    /// The strip the sidebar reserves above its first row when it runs the full
    /// height of the window. The traffic lights are drawn into its left end by
    /// the system; the sync control takes the space beside them, so it sits on
    /// the blur rather than stranded on the opaque title bar to the right.
    fn render_sidebar_title_band(
        &mut self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !full_height_column() {
            return None;
        }
        let sync_control = self.render_title_bar_sync_control(t, cx);
        Some(
            div()
                .w_full()
                .h(px(content_top_inset()))
                .flex_shrink_0()
                .flex()
                .items_center()
                // Pinned to the trailing edge, not stacked against the traffic
                // lights: the lights are a fixed-width cluster at the leading
                // edge and anything crowding them reads as part of them.
                .justify_end()
                .pl(px(traffic_light_clearance()))
                .children(sync_control)
                .into_any_element(),
        )
    }

    /// The strip along the sidebar's trailing edge that drags it wider or
    /// narrower. It sits over the sidebar's own right border rather than taking
    /// layout width of its own, so the sidebar's content box is unaffected.
    pub(crate) fn render_sidebar_resize_handle(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !resizable() {
            return None;
        }
        let t = self.theme();
        let dragging = self.sidebar_resize.is_some();
        Some(
            div()
                .id("sidebar-resize-handle")
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(-RESIZE_HANDLE_WIDTH / 2.0))
                .w(px(RESIZE_HANDLE_WIDTH))
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .when(dragging, |handle| handle.bg(token_rgba(t.border_overlay)))
                .when(!dragging, |handle| {
                    handle.hover(move |h| h.bg(token_rgba(t.border_overlay)))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.begin_sidebar_resize(f32::from(event.position.x));
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    fn render_sidebar_tree(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Inline rename errors make a row taller than nav_row_height(). Keep the
        // recursive renderer while renaming so the input can measure naturally;
        // the normal navigator has exact fixed sizes and can be virtualized.
        if self.rename_node.is_some() {
            return div()
                .id("sidebar-tree-recursive")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(self.sidebar_scroll_handle.base_handle())
                .child(self.render_node_children(self.workspace.root, 0, cx))
                .into_any_element();
        }

        let revision = self.state.schedule_revision();
        if self
            .sidebar_navigator_cache
            .as_ref()
            .is_none_or(|cache| !cache.matches(revision))
        {
            let rows = self.flatten_navigator_rows();
            self.sidebar_navigator_cache = Some(SidebarNavigatorCache::new(revision, rows));
        }
        let (rows_for_render, item_sizes) = self
            .sidebar_navigator_cache
            .as_ref()
            .expect("sidebar navigator cache was initialized")
            .handles();

        v_virtual_list(
            cx.entity(),
            "sidebar-tree-virtual",
            item_sizes,
            move |this: &mut KnotQApp,
                  visible_range: std::ops::Range<usize>,
                  _window: &mut Window,
                  cx: &mut Context<KnotQApp>| {
                visible_range
                    .map(|index| this.render_navigator_row(rows_for_render[index], cx))
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.sidebar_scroll_handle)
        .size_full()
        .into_any_element()
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
