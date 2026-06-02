use super::*;

impl KnotQApp {
    pub(crate) fn open_sidebar_context_menu(
        &mut self,
        target: SidebarContextTarget,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_context_menu = Some(SidebarContextMenu { target, position });
        cx.notify();
    }

    fn close_sidebar_context_menu(&mut self, cx: &mut Context<Self>) {
        self.sidebar_context_menu = None;
        self.google_calendar_picker = None;
        self.google_calendar_picker_task = None;
        cx.notify();
    }

    pub fn render_sidebar_context_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.sidebar_context_menu.clone()?;
        let t = self.theme();
        let root = self.workspace.root;
        let mut items: Vec<gpui::AnyElement> = Vec::new();

        match menu.target {
            SidebarContextTarget::Background => {
                items.push(sidebar_context_item(
                    "sidebar-menu-new-root-item",
                    "New Item",
                    t,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.open_new_node_prompt(root, NewNodeKind::Scheme, window, cx);
                    }),
                ));
                items.push(sidebar_context_item(
                    "sidebar-menu-new-folder",
                    "New Folder",
                    t,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.open_new_node_prompt(root, NewNodeKind::Folder, window, cx);
                    }),
                ));
            }
            SidebarContextTarget::NewMenu { parent } => {
                items.push(sidebar_context_item_with_shortcut(
                    "sidebar-menu-new-item",
                    "Item",
                    Some("⌘N"),
                    t,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.open_new_node_prompt(parent, NewNodeKind::Scheme, window, cx);
                    }),
                ));
                items.push(sidebar_context_item_with_shortcut(
                    "sidebar-menu-new-folder",
                    "Folder",
                    Some("⇧⌘N"),
                    t,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.open_new_node_prompt(parent, NewNodeKind::Folder, window, cx);
                    }),
                ));
                items.push(sidebar_context_separator(t));
                items.push(sidebar_context_item(
                    "sidebar-menu-new-google-calendar",
                    "Google Calendar",
                    t,
                    cx.listener(move |this, event: &ClickEvent, _window, cx| {
                        this.open_google_calendar_picker(parent, event.position(), cx);
                    }),
                ));
            }
            SidebarContextTarget::GoogleCalendarPicker { parent } => {
                items.extend(self.render_google_calendar_picker_items(parent, t, cx));
            }
            SidebarContextTarget::Archive => {
                items.push(sidebar_context_item(
                    "sidebar-menu-empty-archive",
                    "Empty Archive",
                    t,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.request_empty_archive_confirmation(cx);
                    }),
                ));
            }
            SidebarContextTarget::Folder(folder_id) => {
                items.push(sidebar_context_item(
                    "sidebar-menu-new-item",
                    "New Item",
                    t,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.open_new_node_prompt(folder_id, NewNodeKind::Scheme, window, cx);
                    }),
                ));
                items.push(sidebar_context_item(
                    "sidebar-menu-new-folder",
                    "New Folder",
                    t,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.open_new_node_prompt(folder_id, NewNodeKind::Folder, window, cx);
                    }),
                ));
                items.push(sidebar_context_separator(t));
                items.push(sidebar_context_item(
                    "sidebar-menu-rename-folder",
                    "Rename",
                    t,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.start_renaming_node(NodeRef::Folder(folder_id), window, cx);
                    }),
                ));
                items.push(sidebar_context_item(
                    "sidebar-menu-delete-folder",
                    "Archive",
                    t,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.request_delete_folder(folder_id, cx);
                    }),
                ));
            }
            SidebarContextTarget::Scheme { scheme_id } => {
                items.push(sidebar_context_item(
                    "sidebar-menu-rename-scheme",
                    "Rename",
                    t,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.start_renaming_node(NodeRef::Scheme(scheme_id), window, cx);
                    }),
                ));
                items.push(sidebar_context_separator(t));
                items.push(sidebar_context_item(
                    "sidebar-menu-delete-scheme",
                    "Archive",
                    t,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.request_delete_scheme(scheme_id, cx);
                    }),
                ));
            }
            SidebarContextTarget::DeletedScheme { scheme_id } => {
                items.push(sidebar_context_item(
                    "sidebar-menu-restore-deleted-scheme",
                    "Restore",
                    t,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.restore_deleted_scheme(scheme_id, cx);
                    }),
                ));
                items.push(sidebar_context_separator(t));
                items.push(sidebar_context_item(
                    "sidebar-menu-permanently-delete-scheme",
                    "Permanently Delete",
                    t,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.close_sidebar_context_menu(cx);
                        this.permanently_delete_scheme(scheme_id, cx);
                    }),
                ));
            }
        }
        let viewport_width = px(f32::from(window.viewport_size().width));
        let viewport_height = px(f32::from(window.viewport_size().height));
        let menu_width = px(match menu.target {
            SidebarContextTarget::GoogleCalendarPicker { .. } => 260.0,
            _ => 220.0,
        });
        let menu_height = px((items.len() as f32 * 29.0 + 8.0).min(360.0));
        let menu_left = clamped_popover_left(menu.position.x, menu_width, viewport_width);
        let menu_top = popover_top_biased_below(menu.position.y, menu_height, viewport_height);

        Some(
            div()
                .id("sidebar-context-menu-scrim")
                .absolute()
                .inset_0()
                .bg(token_rgba(0x00000000))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.close_sidebar_context_menu(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.close_sidebar_context_menu(cx);
                    }),
                )
                .child(
                    div()
                        .id("sidebar-context-menu")
                        .absolute()
                        .left(menu_left)
                        .top(menu_top)
                        .occlude()
                        .min_w(menu_width)
                        .max_h(px(360.0))
                        .overflow_y_scroll()
                        .p(px(4.0))
                        .rounded(px(7.0))
                        .bg(token_hsla(t.bg_sidebar))
                        .border_1()
                        .border_color(token_rgba(t.border_overlay))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .children(items),
                )
                .into_any_element(),
        )
    }

    fn render_google_calendar_picker_items(
        &mut self,
        parent: FolderId,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let status = self
            .google_calendar_picker
            .as_ref()
            .filter(|picker| picker.parent == parent)
            .map(|picker| picker.status.clone())
            .unwrap_or(GoogleCalendarPickerStatus::Loading);
        let mut items = Vec::new();

        match status {
            GoogleCalendarPickerStatus::Loading => {
                items.push(sidebar_context_disabled_item(
                    "sidebar-google-calendar-loading",
                    "Loading calendars...",
                    None::<SharedString>,
                    t,
                ));
            }
            GoogleCalendarPickerStatus::Error(message) => {
                items.push(sidebar_context_disabled_item(
                    "sidebar-google-calendar-error",
                    message,
                    None::<SharedString>,
                    t,
                ));
                items.push(sidebar_context_separator(t));
                items.push(add_google_account_item(parent, t, cx));
            }
            GoogleCalendarPickerStatus::Loaded { accounts } => {
                if accounts.is_empty() {
                    items.push(sidebar_context_disabled_item(
                        "sidebar-google-calendar-empty",
                        "No local Google accounts",
                        None::<SharedString>,
                        t,
                    ));
                } else {
                    for (account_idx, account) in accounts.into_iter().enumerate() {
                        if account_idx > 0 {
                            items.push(sidebar_context_separator(t));
                        }
                        items.push(sidebar_context_subheading(
                            format!("sidebar-google-calendar-account-{account_idx}"),
                            account.label.clone(),
                            t,
                        ));
                        if let Some(error) = account.error {
                            items.push(sidebar_context_disabled_item(
                                format!("sidebar-google-calendar-account-error-{account_idx}"),
                                error,
                                None::<SharedString>,
                                t,
                            ));
                        } else if account.calendars.is_empty() {
                            items.push(sidebar_context_disabled_item(
                                format!("sidebar-google-calendar-account-empty-{account_idx}"),
                                "No calendars available",
                                None::<SharedString>,
                                t,
                            ));
                        } else {
                            for (calendar_idx, calendar) in
                                account.calendars.into_iter().enumerate()
                            {
                                let id =
                                    format!("sidebar-google-calendar-{account_idx}-{calendar_idx}");
                                if calendar.already_added {
                                    items.push(sidebar_context_disabled_item(
                                        id,
                                        calendar.label,
                                        Some("Added"),
                                        t,
                                    ));
                                } else {
                                    let account_id = account.account_id.clone();
                                    let calendar_id = calendar.id.clone();
                                    items.push(sidebar_context_item(
                                        id,
                                        calendar.label,
                                        t,
                                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                            this.close_sidebar_context_menu(cx);
                                            this.start_google_calendar_import_calendar(
                                                parent,
                                                account_id.clone(),
                                                calendar_id.clone(),
                                                cx,
                                            );
                                        }),
                                    ));
                                }
                            }
                        }
                    }
                }
                items.push(sidebar_context_separator(t));
                items.push(add_google_account_item(parent, t, cx));
            }
        }
        items
    }
}

fn add_google_account_item(
    parent: FolderId,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    sidebar_context_item(
        "sidebar-google-calendar-add-account",
        "Add Google Account...",
        t,
        cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.close_sidebar_context_menu(cx);
            this.start_google_calendar_import(parent, cx);
        }),
    )
}

fn sidebar_context_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    sidebar_context_item_with_shortcut(id, label, None::<SharedString>, t, on_click)
}

fn sidebar_context_item_with_shortcut(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    shortcut: Option<impl Into<SharedString>>,
    t: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    let label = label.into();
    let mut row = div()
        .id(id.into())
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .text_size(px(13.0))
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(token_hsla(t.text_primary))
        .cursor_pointer()
        .hover(move |s| s.bg(token_rgba(t.row_hover_strong)))
        .on_click(on_click)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .whitespace_nowrap()
                .child(label),
        );
    if let Some(shortcut) = shortcut {
        row = row.child(
            div()
                .ml(px(12.0))
                .text_size(px(11.0))
                .text_color(token_hsla(t.text_muted))
                .child(shortcut.into()),
        );
    }
    row.into_any_element()
}

fn sidebar_context_disabled_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    detail: Option<impl Into<SharedString>>,
    t: Theme,
) -> gpui::AnyElement {
    let label = label.into();
    let mut row = div()
        .id(id.into())
        .min_h(px(28.0))
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .font_family(FONT_UI)
        .text_color(token_hsla(t.text_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .whitespace_nowrap()
                .child(label),
        );
    if let Some(detail) = detail {
        row = row.child(
            div()
                .ml(px(12.0))
                .text_size(px(11.0))
                .text_color(token_hsla(t.text_dim))
                .child(detail.into()),
        );
    }
    row.into_any_element()
}

fn sidebar_context_subheading(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: Theme,
) -> gpui::AnyElement {
    div()
        .id(id.into())
        .px(px(10.0))
        .pt(px(6.0))
        .pb(px(3.0))
        .text_size(px(11.0))
        .line_height(px(14.0))
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(token_hsla(t.text_dim))
        .child(label.into())
        .into_any_element()
}

fn sidebar_context_separator(t: Theme) -> gpui::AnyElement {
    div()
        .h(px(1.0))
        .mx(px(4.0))
        .my(px(3.0))
        .bg(token_rgba(t.divider_soft))
        .into_any_element()
}
